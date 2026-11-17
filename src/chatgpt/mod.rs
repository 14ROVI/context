//! ChatGPT plan access through Sign in with ChatGPT and the Responses API.
//! All calls block, so they run on the worker thread in `desktop`.

mod auth;
mod responses;

use std::error::Error;
use std::fmt;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::redirect::Policy;

use self::auth::Settings;
use crate::Result;
use crate::capture::Snapshot;
use crate::sources::Sources;

pub use self::responses::Answer;

const AUTH_ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";

/// Returned when the user must connect, or reconnect, ChatGPT before continuing.
#[derive(Debug)]
pub struct SignInRequired(pub &'static str);

impl fmt::Display for SignInRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for SignInRequired {}

fn sign_in_required(message: &'static str) -> Box<dyn Error + Send + Sync> {
    Box::new(SignInRequired(message))
}

pub struct ChatGpt {
    http: Client,
    settings: Option<Settings>,
    models: Option<Vec<String>>,
}

impl ChatGpt {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .redirect(Policy::none())
            .build()?;
        Ok(Self {
            http,
            settings: None,
            models: None,
        })
    }

    /// The connected account's email, or `None` when signed out.
    pub fn email(&mut self) -> Result<Option<String>> {
        Ok(self.settings()?.email())
    }

    /// Opens the browser sign-in and waits for it. Returns the account email.
    pub fn sign_in(&mut self) -> Result<String> {
        let email = auth::sign_in(&self.http, load(&mut self.settings)?)?;
        self.models = None;
        Ok(email)
    }

    pub fn sign_out(&mut self) -> Result<String> {
        let status = auth::sign_out(&self.http, load(&mut self.settings)?)?;
        self.models = None;
        Ok(status)
    }

    /// Fails early with [`SignInRequired`] so nothing is captured while signed out.
    pub fn ensure_connected(&mut self) -> Result<()> {
        let settings = self.settings()?;
        settings
            .account
            .as_ref()
            .filter(|account| account.session.is_some())
            .ok_or_else(|| sign_in_required("Connect ChatGPT to start using Context."))?
            .check_plan()
    }

    /// Sends one capture to the model, which may consult `sources` first.
    /// `progress` receives short status lines such as "Checking calendar…".
    pub fn analyze(
        &mut self,
        snapshot: &Snapshot,
        sources: &Sources,
        progress: &dyn Fn(String),
    ) -> Result<Answer> {
        self.ensure_connected()?;
        let http = self.http.clone();
        let settings = self.settings()?;
        auth::refresh_if_needed(&http, settings)?;
        let access_token = settings
            .account
            .as_ref()
            .and_then(|account| account.session.as_ref())
            .map(|session| session.access_token.clone())
            .ok_or_else(|| sign_in_required("Connect ChatGPT to start using Context."))?;
        let models = match &self.models {
            Some(models) => models,
            None => self
                .models
                .insert(responses::list_models(&http, &access_token)?),
        };
        let model = responses::choose_model(models)?;
        crate::debug(format_args!("models listed: {models:?}; using {model}"));
        responses::analyze(&http, &access_token, model, snapshot, sources, progress)
    }

    fn settings(&mut self) -> Result<&mut Settings> {
        load(&mut self.settings)
    }
}

/// Reads saved settings the first time they are needed.
fn load(settings: &mut Option<Settings>) -> Result<&mut Settings> {
    if settings.is_none() {
        *settings = Some(Settings::load()?);
    }
    Ok(settings.as_mut().expect("settings were loaded above"))
}
