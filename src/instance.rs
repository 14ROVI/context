//! Single-instance handling and the `context <command>` remote.
//!
//! The first Context process holds a file lock and listens on a loopback port.
//! Later launches pass their command to it and exit. Desktops without a global
//! shortcut API, such as Wayland compositors, can bind `context capture` instead.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::thread;
use std::time::Duration;

use uuid::Uuid;

use crate::{Result, storage};

const ADDRESS_FILE: &str = "instance.addr";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remote {
    Open,
    Capture,
}

impl Remote {
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "open" => Some(Self::Open),
            "capture" => Some(Self::Capture),
            _ => None,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Capture => "capture",
        }
    }
}

pub enum Launch {
    /// This process owns the instance and should start the app.
    Primary(Instance),
    /// Another Context process accepted the command.
    Forwarded,
}

pub struct Instance {
    _lock: File,
    listener: TcpListener,
    token: String,
}

pub fn claim(command: Remote) -> Result<Launch> {
    let directory = storage::data_dir()?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("instance.lock"))?;
    if lock.try_lock().is_err() {
        forward(&directory, command)?;
        return Ok(Launch::Forwarded);
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let token = Uuid::new_v4().simple().to_string();
    fs::write(
        directory.join(ADDRESS_FILE),
        format!("{} {token}", listener.local_addr()?),
    )?;
    Ok(Launch::Primary(Instance {
        _lock: lock,
        listener,
        token,
    }))
}

impl Instance {
    /// Calls `handle` on a background thread for each command another launch sends.
    pub fn listen(self, handle: impl Fn(Remote) + Send + 'static) -> Result<File> {
        let Instance {
            _lock,
            listener,
            token,
        } = self;
        thread::Builder::new()
            .name("context-instance".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    if let Some(command) = read_command(stream, &token) {
                        handle(command);
                    }
                }
            })?;
        Ok(_lock)
    }
}

fn read_command(stream: TcpStream, token: &str) -> Option<Remote> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut line = String::new();
    BufReader::new(stream.take(256)).read_line(&mut line).ok()?;
    let (sent, word) = line.trim().split_once(' ')?;
    (sent == token).then(|| Remote::parse(word)).flatten()
}

fn forward(directory: &Path, command: Remote) -> Result<()> {
    let unavailable = "Context is already running but did not respond. Quit it and try again.";
    let saved = fs::read_to_string(directory.join(ADDRESS_FILE)).map_err(|_| unavailable)?;
    let (address, token) = saved.trim().split_once(' ').ok_or(unavailable)?;
    let address: SocketAddr = address.parse().map_err(|_| unavailable)?;
    if !address.ip().is_loopback() {
        return Err(unavailable.into());
    }
    let mut stream =
        TcpStream::connect_timeout(&address, Duration::from_secs(2)).map_err(|_| unavailable)?;
    stream.write_all(format!("{token} {}\n", command.word()).as_bytes())?;
    Ok(())
}
