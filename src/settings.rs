//! Preferences people change in the Settings page, saved as `settings.json` in
//! the config folder. Secrets such as API keys never go here; they are kept as
//! encrypted records instead.

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::storage;

const FILE: &str = "settings.json";
pub const DEFAULT_SHORTCUT: &str = "Ctrl+Alt+Space";

/// Which service answers questions about the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    /// A ChatGPT plan, through Sign in with ChatGPT.
    #[default]
    ChatGpt,
    /// The OpenAI API, with the user's API key.
    OpenAi,
    /// The Anthropic API, with the user's API key.
    Anthropic,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// The global capture shortcut, such as `Ctrl+Alt+Space`.
    pub shortcut: String,
    pub provider: Provider,
    /// A model the provider lists. `None` picks Context's default.
    pub model: Option<String>,
    pub start_at_login: bool,
    /// Keep the screenshot behind each saved result.
    pub save_screenshots: bool,
    /// Delete results older than this many days. 0 keeps them forever.
    pub history_days: u32,
    /// Apps Context never captures, matched against the focused app's name.
    pub excluded_apps: Vec<String>,
    /// Experimental: watch the screen and offer cards without the shortcut.
    pub auto_cards: bool,
    /// Connectors built into Context that the user added, such as `google-calendar`.
    pub connectors: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shortcut: DEFAULT_SHORTCUT.into(),
            provider: Provider::default(),
            model: None,
            start_at_login: false,
            save_screenshots: true,
            history_days: 0,
            excluded_apps: Vec::new(),
            auto_cards: false,
            connectors: Vec::new(),
        }
    }
}

impl Settings {
    /// Reads `settings.json`. A missing or unreadable file gives the defaults.
    pub fn load() -> Self {
        storage::config_dir()
            .ok()
            .and_then(|dir| std::fs::read_to_string(dir.join(FILE)).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = storage::config_dir()?.join(FILE);
        std::fs::write(path, serde_json::to_string_pretty(self)? + "\n")?;
        Ok(())
    }

    /// Whether `app` is on the excluded list. Matching ignores case and
    /// accepts part of the name, so "slack" excludes "Slack".
    pub fn excludes(&self, app: &str) -> bool {
        let app = app.to_lowercase();
        self.excluded_apps
            .iter()
            .map(|name| name.trim().to_lowercase())
            .any(|name| !name.is_empty() && app.contains(&name))
    }
}
