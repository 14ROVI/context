//! Encrypted local records.
//!
//! Each record is a JSON document sealed with AES-256-GCM. The random key lives in
//! the operating system's credential store through `keyring`: Credential Manager
//! on Windows, the login Keychain on macOS and the Secret Service on Linux.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use directories::ProjectDirs;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Serialize, de::DeserializeOwned};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::Result;

pub const APP_ID: &str = "com.context.desktop";
const KEY_NAME: &str = "local-data-key";
const FORMAT: &[u8; 4] = b"CTX2";
const NONCE_LEN: usize = 12;
/// Screenshots live in this subfolder, one encrypted file per history entry.
const SCREENSHOTS: &str = "screenshots";

/// Caches the key so the credential store is asked at most once per run.
static KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);

#[derive(Clone, Copy)]
pub enum Record {
    Account,
    History,
    /// The JPEG captured for one history entry.
    Screenshot(Uuid),
    OpenAiKey,
    AnthropicKey,
    /// The OAuth grant for one source, keyed by [`key`].
    SourceGrant(u64),
    /// A secret setting for one source, keyed by [`key`].
    SourceSecret(u64),
}

/// A stable key for a record named by text, such as a source's address.
pub fn key(text: &str) -> u64 {
    let digest = ring::digest::digest(&ring::digest::SHA256, text.as_bytes());
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&digest.as_ref()[..8]);
    u64::from_be_bytes(bytes)
}

impl Record {
    /// The path inside the data folder. It also binds the ciphertext to its
    /// record, so one record cannot be swapped for another.
    fn file_name(self) -> String {
        match self {
            Record::Account => "account.ctx".into(),
            Record::History => "history.ctx".into(),
            Record::Screenshot(id) => format!("{SCREENSHOTS}/{}.ctx", id.simple()),
            Record::OpenAiKey => "openai-key.ctx".into(),
            Record::AnthropicKey => "anthropic-key.ctx".into(),
            Record::SourceGrant(key) => format!("sources/grant-{key:016x}.ctx"),
            Record::SourceSecret(key) => format!("sources/secret-{key:016x}.ctx"),
        }
    }

    fn max_bytes(self) -> u64 {
        match self {
            Record::Screenshot(_) => 8 * 1024 * 1024,
            _ => 512 * 1024,
        }
    }
}

/// The per-user folder for Context data, created on first use.
pub fn data_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "Context")
        .ok_or("Context could not find a folder for its data.")?;
    let path = dirs.data_local_dir().to_owned();
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// The per-user folder for settings people may edit, such as `mcp.json`.
pub fn config_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "Context")
        .ok_or("Context could not find a folder for its settings.")?;
    let path = dirs.config_dir().to_owned();
    fs::create_dir_all(&path)?;
    Ok(path)
}

pub fn load<T: DeserializeOwned>(record: Record) -> Result<Option<T>> {
    match load_bytes(record)? {
        Some(plain) => Ok(Some(serde_json::from_slice(&plain)?)),
        None => Ok(None),
    }
}

pub fn save<T: Serialize>(record: Record, value: &T) -> Result<()> {
    save_bytes(record, serde_json::to_vec(value)?)
}

pub fn load_bytes(record: Record) -> Result<Option<Vec<u8>>> {
    let file = match File::open(data_dir()?.join(record.file_name())) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut sealed = Vec::new();
    file.take(record.max_bytes() + 1).read_to_end(&mut sealed)?;
    if sealed.len() as u64 > record.max_bytes() {
        return Err("A saved Context record is too large to open.".into());
    }
    Ok(Some(open_record(record, &sealed)?))
}

pub fn save_bytes(record: Record, plain: Vec<u8>) -> Result<()> {
    let path = data_dir()?.join(record.file_name());
    let directory = path
        .parent()
        .ok_or("Context could not find a folder for its data.")?;
    fs::create_dir_all(directory)?;
    let sealed = seal_record(record, plain)?;
    if sealed.len() as u64 > record.max_bytes() {
        return Err("This Context record is too large to save.".into());
    }
    // Write beside the target and rename so a crash never leaves half a record.
    let mut file = NamedTempFile::new_in(directory)?;
    file.write_all(&sealed)?;
    file.as_file().sync_all()?;
    file.persist(&path)?;
    Ok(())
}

/// Deletes a record. A record that is already gone is not an error.
pub fn remove(record: Record) -> Result<()> {
    match fs::remove_file(data_dir()?.join(record.file_name())) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

fn seal_record(record: Record, mut data: Vec<u8>) -> Result<Vec<u8>> {
    let key = cipher(true)?;
    let mut nonce = [0; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| "Context could not encrypt its data.")?;
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(record.file_name().as_bytes()),
        &mut data,
    )
    .map_err(|_| "Context could not encrypt its data.")?;
    Ok([FORMAT.as_slice(), &nonce, &data].concat())
}

fn open_record(record: Record, sealed: &[u8]) -> Result<Vec<u8>> {
    let payload = sealed
        .strip_prefix(FORMAT)
        .ok_or("A saved Context record has an unknown format.")?;
    let (nonce, ciphertext) = payload
        .split_first_chunk::<NONCE_LEN>()
        .ok_or("A saved Context record is incomplete.")?;
    let mut ciphertext = ciphertext.to_vec();
    let plain = cipher(false)?
        .open_in_place(
            Nonce::assume_unique_for_key(*nonce),
            Aad::from(record.file_name().as_bytes()),
            &mut ciphertext,
        )
        .map_err(|_| "A saved Context record could not be decrypted.")?;
    Ok(plain.to_vec())
}

fn cipher(create: bool) -> Result<LessSafeKey> {
    let mut cached = KEY
        .lock()
        .map_err(|_| "The Context key cache is unavailable.")?;
    let key = match *cached {
        Some(key) => key,
        None => {
            let key = keyring_key(create)?;
            *cached = Some(key);
            key
        }
    };
    let key = UnboundKey::new(&AES_256_GCM, &key)
        .map_err(|_| "The Context key in your credential store is invalid.")?;
    Ok(LessSafeKey::new(key))
}

fn keyring_key(create: bool) -> Result<[u8; 32]> {
    let entry = keyring::Entry::new(APP_ID, KEY_NAME)?;
    match entry.get_secret() {
        Ok(secret) => Ok(secret
            .try_into()
            .map_err(|_| "The Context key in your credential store is invalid.")?),
        Err(keyring::Error::NoEntry) if create => {
            let mut key = [0; 32];
            SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| "Context could not create an encryption key.")?;
            entry.set_secret(&key)?;
            Ok(key)
        }
        Err(keyring::Error::NoEntry) => {
            Err("The Context key is missing from your credential store.".into())
        }
        Err(error) => {
            Err(format!("Context could not reach your system credential store. {error}").into())
        }
    }
}
