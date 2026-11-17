//! A ChatGPT plan through Sign in with ChatGPT. Requests go through the
//! Responses API in `openai.rs` with the plan's access token.

mod auth;

use std::error::Error;

use reqwest::blocking::Client;

use self::auth::Settings;
use super::{Fix, needs};
use crate::Result;

const AUTH_ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";

fn sign_in_required(message: &'static str) -> Box<dyn Error + Send + Sync> {
    needs(Fix::SignIn, message)
}

pub struct ChatGpt {
    http: Client,
    settings: Option<Settings>,
}

impl ChatGpt {
    pub fn new(http: Client) -> Self {
        Self {
            http,
            settings: None,
        }
    }

    /// The connected account's email, or `None` when signed out.
    pub fn email(&mut self) -> Result<Option<String>> {
        Ok(self.settings()?.email())
    }

    /// Opens the browser sign-in and waits for it. Returns the account email.
    pub fn sign_in(&mut self) -> Result<String> {
        auth::sign_in(&self.http, load(&mut self.settings)?)
    }

    pub fn sign_out(&mut self) -> Result<String> {
        auth::sign_out(&self.http, load(&mut self.settings)?)
    }

    /// Fails early with a sign-in action so nothing is captured while signed out.
    pub fn ensure_connected(&mut self) -> Result<()> {
        let settings = self.settings()?;
        settings
            .account
            .as_ref()
            .filter(|account| account.session.is_some())
            .ok_or_else(|| sign_in_required("Connect ChatGPT to start using Context."))?
            .check_plan()
    }

    /// A current access token, refreshed first when it is about to expire.
    pub fn access_token(&mut self) -> Result<String> {
        self.ensure_connected()?;
        let http = self.http.clone();
        let settings = self.settings()?;
        auth::refresh_if_needed(&http, settings)?;
        settings
            .account
            .as_ref()
            .and_then(|account| account.session.as_ref())
            .map(|session| session.access_token.clone())
            .ok_or_else(|| sign_in_required("Connect ChatGPT to start using Context."))
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
