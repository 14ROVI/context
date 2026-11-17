//! Asking a model about a capture. `Assistant` hands the request to the
//! provider chosen in Settings: a ChatGPT plan, an OpenAI API key, or an
//! Anthropic API key. All calls block, so they run on the worker thread.

mod anthropic;
mod chatgpt;
mod openai;
mod prompt;

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::redirect::Policy;

use self::chatgpt::ChatGpt;
use crate::Result;
use crate::capture::Snapshot;
use crate::settings::{Provider, Settings};
use crate::sources::Sources;
use crate::storage::{self, Record};

/// What the user can do to fix a failed request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Connect, or reconnect, the ChatGPT plan.
    SignIn,
    /// Add or change something in Settings, such as an API key.
    Settings,
    /// Check plan or account usage at this address.
    Usage(&'static str),
    /// The focused app is on the user's excluded list.
    Excluded,
}

/// A failure the user can fix, with what to do about it.
#[derive(Debug)]
pub struct NeedsAction {
    pub message: String,
    pub fix: Fix,
}

impl fmt::Display for NeedsAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for NeedsAction {}

pub fn needs(fix: Fix, message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(NeedsAction {
        message: message.into(),
        fix,
    })
}

/// What to ask the model: instructions, text and an optional screenshot.
pub struct Request<'a> {
    pub instructions: String,
    pub text: String,
    pub image: Option<&'a [u8]>,
}

impl<'a> Request<'a> {
    /// Asks about a fresh capture. `quiet` raises the bar for automatic checks.
    pub fn capture(snapshot: &'a Snapshot, quiet: bool) -> Self {
        let mut instructions = prompt::INSTRUCTIONS.to_owned();
        if quiet {
            instructions.push_str(prompt::QUIET);
        }
        Self {
            instructions,
            text: prompt::desktop_metadata(snapshot),
            image: Some(&snapshot.jpeg),
        }
    }

    /// Asks a follow-up about an earlier answer, with the screenshot it came
    /// from when one was saved.
    pub fn follow_up(earlier: &str, question: &str, image: Option<&'a [u8]>) -> Self {
        Self {
            instructions: prompt::FOLLOW_UP.to_owned(),
            text: format!(
                "Earlier conversation:
{earlier}

Follow-up question from the user:
{question}"
            ),
            image,
        }
    }
}

/// The model's final answer and what it consulted on the way.
pub struct Answer {
    /// `None` when the model had nothing useful to add.
    pub text: Option<String>,
    pub sources: Vec<String>,
    /// A kind of source the model said would have helped, such as `calendar`.
    pub needs: Option<String>,
}

pub struct Assistant {
    http: Client,
    chatgpt: ChatGpt,
    models: HashMap<Provider, Vec<String>>,
}

impl Assistant {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(90))
            .redirect(Policy::none())
            .build()?;
        Ok(Self {
            chatgpt: ChatGpt::new(http.clone()),
            http,
            models: HashMap::new(),
        })
    }

    /// The connected ChatGPT account's email, or `None` when signed out.
    pub fn chatgpt_email(&mut self) -> Result<Option<String>> {
        self.chatgpt.email()
    }

    pub fn sign_in(&mut self) -> Result<String> {
        self.models.remove(&Provider::ChatGpt);
        self.chatgpt.sign_in()
    }

    pub fn sign_out(&mut self) -> Result<String> {
        self.models.remove(&Provider::ChatGpt);
        self.chatgpt.sign_out()
    }

    /// Saves or removes the API key for `provider`. Keys are encrypted at rest
    /// and never sent back to the pages.
    pub fn set_api_key(&mut self, provider: Provider, key: Option<String>) -> Result<()> {
        let record = key_record(provider).ok_or("This provider does not use an API key.")?;
        self.models.remove(&provider);
        match key
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
        {
            Some(key) => storage::save(record, &key),
            None => storage::remove(record),
        }
    }

    pub fn has_api_key(provider: Provider) -> bool {
        key_record(provider)
            .is_some_and(|record| storage::load::<String>(record).ok().flatten().is_some())
    }

    /// Fails early, before anything is captured, when the chosen provider is
    /// not set up.
    pub fn ensure_ready(&mut self, settings: &Settings) -> Result<()> {
        match settings.provider {
            Provider::ChatGpt => self.chatgpt.ensure_connected(),
            provider => api_key(provider).map(|_| ()),
        }
    }

    /// Models the chosen provider lists, for the Settings page.
    pub fn models(&mut self, provider: Provider) -> Result<Vec<String>> {
        if let Some(models) = self.models.get(&provider) {
            return Ok(models.clone());
        }
        let models = match provider {
            Provider::ChatGpt => {
                let token = self.chatgpt.access_token()?;
                openai::list_models(&self.http, &token, openai::Service::Plan)?
            }
            Provider::OpenAi => {
                openai::list_models(&self.http, &api_key(provider)?, openai::Service::ApiKey)?
            }
            Provider::Anthropic => anthropic::list_models(&self.http, &api_key(provider)?)?,
        };
        self.models.insert(provider, models.clone());
        Ok(models)
    }

    /// Sends a request to the model, which may consult `sources` first.
    /// `progress` receives short status lines such as "Checking calendar…".
    pub fn ask(
        &mut self,
        request: &Request,
        sources: &Sources,
        progress: &dyn Fn(String),
        settings: &Settings,
    ) -> Result<Answer> {
        let preferred = settings.model.as_deref();
        match settings.provider {
            Provider::ChatGpt => {
                let token = self.chatgpt.access_token()?;
                let models = self.models(Provider::ChatGpt)?;
                let model = openai::choose_model(&models, preferred, openai::Service::Plan)?;
                crate::debug(format_args!(
                    "ChatGPT plan models: {models:?}; using {model}"
                ));
                openai::analyze(
                    &self.http,
                    &token,
                    &model,
                    request,
                    sources,
                    progress,
                    openai::Service::Plan,
                )
            }
            Provider::OpenAi => {
                let key = api_key(Provider::OpenAi)?;
                let models = self.models(Provider::OpenAi)?;
                let model = openai::choose_model(&models, preferred, openai::Service::ApiKey)?;
                crate::debug(format_args!("OpenAI API: using {model}"));
                openai::analyze(
                    &self.http,
                    &key,
                    &model,
                    request,
                    sources,
                    progress,
                    openai::Service::ApiKey,
                )
            }
            Provider::Anthropic => {
                let key = api_key(Provider::Anthropic)?;
                let model = preferred.unwrap_or(anthropic::DEFAULT_MODEL);
                crate::debug(format_args!("Anthropic API: using {model}"));
                anthropic::analyze(&self.http, &key, model, request, sources, progress)
            }
        }
    }
}

fn key_record(provider: Provider) -> Option<Record> {
    match provider {
        Provider::ChatGpt => None,
        Provider::OpenAi => Some(Record::OpenAiKey),
        Provider::Anthropic => Some(Record::AnthropicKey),
    }
}

fn api_key(provider: Provider) -> Result<String> {
    let name = match provider {
        Provider::OpenAi => "OpenAI",
        _ => "Anthropic",
    };
    key_record(provider)
        .and_then(|record| storage::load::<String>(record).ok().flatten())
        .ok_or_else(|| {
            needs(
                Fix::Settings,
                format!("Add your {name} API key in Settings."),
            )
        })
}
