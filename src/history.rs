//! Saved results. The newest entries are kept encrypted on disk.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::storage::{self, Record};

const MAX_ENTRIES: usize = 100;

#[derive(Clone, Serialize, Deserialize)]
pub struct ContextText {
    pub headline: String,
    pub summary: String,
    pub details: String,
}

impl ContextText {
    /// Splits the model's plain-text answer into headline, summary and details.
    pub fn from_response(response: &str) -> Option<Self> {
        let mut lines = response
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty());
        let headline = shorten(lines.next()?, 96);
        let summary = lines
            .next()
            .map_or_else(String::new, |line| shorten(line, 220));
        let details = shorten(&lines.collect::<Vec<_>>().join("\n\n"), 900);
        Some(Self {
            headline,
            summary,
            details,
        })
    }

    pub fn plain_text(&self) -> String {
        [&self.headline, &self.summary, &self.details]
            .into_iter()
            .filter(|part| !part.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Loads the JPEG saved with an entry, if there is one.
pub fn screenshot(entry: &Entry) -> Option<Vec<u8>> {
    if !entry.has_screenshot {
        return None;
    }
    storage::load_bytes(Record::Screenshot(entry.id))
        .ok()
        .flatten()
}

fn forget_screenshots(entries: &[Entry]) {
    for entry in entries.iter().filter(|entry| entry.has_screenshot) {
        let _ = storage::remove(Record::Screenshot(entry.id));
    }
}

fn shorten(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let mut shortened: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        shortened.push('…');
    }
    shortened
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: Uuid,
    pub created_at: i64,
    /// The app that was in focus, when the desktop reports it.
    #[serde(default)]
    pub app: Option<String>,
    pub text: ContextText,
    /// Connected sources the model consulted, such as `calendar`.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Whether the capture was saved as `Record::Screenshot(id)`.
    #[serde(default)]
    pub has_screenshot: bool,
}

pub struct History {
    pub entries: Vec<Entry>,
    /// Set when saved history cannot be read or written. Entries then last
    /// until Context quits, and the unreadable file is left untouched.
    pub problem: Option<&'static str>,
    writable: bool,
}

impl History {
    pub fn load() -> Self {
        match storage::load::<Vec<Entry>>(Record::History) {
            Ok(entries) => {
                let mut entries = entries.unwrap_or_default();
                entries.truncate(MAX_ENTRIES);
                Self {
                    entries,
                    problem: None,
                    writable: true,
                }
            }
            Err(_) => Self {
                entries: Vec::new(),
                problem: Some(
                    "Saved history could not be opened, so new results last until you quit.",
                ),
                writable: false,
            },
        }
    }

    pub fn push(
        &mut self,
        text: ContextText,
        app: Option<String>,
        sources: Vec<String>,
        screenshot: Option<Vec<u8>>,
    ) -> Entry {
        let id = Uuid::new_v4();
        // Screenshots are only kept when history itself can be saved.
        let has_screenshot = self.writable
            && screenshot
                .is_some_and(|jpeg| storage::save_bytes(Record::Screenshot(id), jpeg).is_ok());
        let entry = Entry {
            id,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| time.as_secs() as i64),
            app,
            text,
            sources,
            has_screenshot,
        };
        self.entries.insert(0, entry.clone());
        if self.entries.len() > MAX_ENTRIES {
            let dropped: Vec<Entry> = self.entries.drain(MAX_ENTRIES..).collect();
            forget_screenshots(&dropped);
        }
        self.persist();
        entry
    }

    pub fn remove(&mut self, id: Uuid) {
        let (removed, kept) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition(|entry| entry.id == id);
        self.entries = kept;
        forget_screenshots(&removed);
        self.persist();
    }

    pub fn clear(&mut self) {
        forget_screenshots(&std::mem::take(&mut self.entries));
        self.persist();
    }

    fn persist(&mut self) {
        if self.writable {
            self.problem = storage::save(Record::History, &self.entries)
                .err()
                .map(|_| "History could not be saved, so recent results last until you quit.");
        }
    }
}
