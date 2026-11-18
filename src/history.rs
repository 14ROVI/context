//! Saved results. The newest entries are kept encrypted on disk.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::storage::{self, Record};

/// A web page the answer cites.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ContextText {
    pub headline: String,
    pub summary: String,
    pub details: String,
}

impl ContextText {
    /// Splits the model's answer into headline, summary and details. The
    /// details keep their line breaks, since the views render them as Markdown.
    pub fn from_response(response: &str) -> Option<Self> {
        /// Takes the next non-empty line and returns it with the text after it.
        fn next_line(text: &str) -> Option<(&str, &str)> {
            let text = text.trim_start();
            if text.is_empty() {
                return None;
            }
            let (line, rest) = text.split_once('\n').unwrap_or((text, ""));
            Some((line.trim(), rest))
        }
        let (headline, rest) = next_line(response)?;
        // The headline is shown as plain text, so drop heading and bold marks.
        let headline = shorten(
            headline.trim_start_matches('#').trim().trim_matches('*'),
            96,
        );
        let (summary, rest) = next_line(rest).unwrap_or(("", ""));
        let summary = shorten(summary, 220);
        let details = shorten(rest.trim(), 900);
        Some(Self {
            headline,
            summary,
            details,
        })
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
    pub text: ContextText,
    /// Connected sources the model consulted, such as `calendar`.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Web pages the answer cites.
    #[serde(default)]
    pub links: Vec<Link>,
    /// A kind of source the model said it lacked, such as `web`.
    #[serde(default)]
    pub needs: Option<String>,
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
                let entries = entries.unwrap_or_default();
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
        sources: Vec<String>,
        links: Vec<Link>,
        needs: Option<String>,
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
            text,
            sources,
            links,
            needs,
            has_screenshot,
        };
        self.entries.insert(0, entry.clone());
        self.persist();
        entry
    }

    /// Deletes results older than `days`. 0 keeps them forever.
    pub fn prune(&mut self, days: u32) {
        if days == 0 {
            return;
        }
        let cutoff = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_secs() as i64)
            - i64::from(days) * 86_400;
        if self.entries.iter().all(|entry| entry.created_at >= cutoff) {
            return;
        }
        let (old, kept) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition(|entry| entry.created_at < cutoff);
        self.entries = kept;
        forget_screenshots(&old);
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
