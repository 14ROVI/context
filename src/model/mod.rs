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
use crate::history::Link;
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

/// What to ask the model: instructions, text and a screenshot.
pub struct Request<'a> {
    pub instructions: String,
    pub text: String,
    pub image: &'a [u8],
}

impl<'a> Request<'a> {
    /// Asks about a fresh capture. `trigger` is the new text that started an
    /// automatic check; those checks get a higher bar for speaking up.
    pub fn capture(snapshot: &'a Snapshot, trigger: Option<&str>) -> Self {
        let mut instructions = prompt::INSTRUCTIONS.to_owned();
        let mut text = prompt::desktop_metadata(snapshot);
        if let Some(trigger) = trigger {
            instructions.push_str(prompt::QUIET);
            text.push_str(&prompt::trigger_note(trigger));
        }
        Self {
            instructions,
            text,
            image: &snapshot.jpeg,
        }
    }
}

/// The model's final answer and what it consulted on the way.
pub struct Answer {
    /// `None` when the model had nothing useful to add.
    pub text: Option<String>,
    pub sources: Vec<String>,
    /// Web pages the answer cites.
    pub links: Vec<Link>,
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
            Provider::Anthropic => anthropic::list_models(&self.http, &api_key(provider)?)?,
            _ => {
                let (token, service) = self.openai_token(provider)?;
                openai::list_models(&self.http, &token, service)?
            }
        };
        self.models.insert(provider, models.clone());
        Ok(models)
    }

    /// Asks the provider's cheapest model whether new on-screen text deserves a
    /// full check. Text only; no screenshot is sent.
    pub fn worth_a_look(&mut self, text: &str, settings: &Settings) -> Result<bool> {
        let reply = match settings.provider {
            Provider::Anthropic => {
                let key = api_key(Provider::Anthropic)?;
                anthropic::quick(&self.http, &key, prompt::GATE, text)?
            }
            provider => {
                let (token, service) = self.openai_token(provider)?;
                let models = self.models(provider)?;
                let model = openai::cheap_model(&models, service)?;
                openai::quick(&self.http, &token, &model, prompt::GATE, text, service)?
            }
        };
        let yes = reply.trim().to_uppercase().starts_with("YES");
        crate::debug(format_args!(
            "automatic cards: gate answered {}",
            reply.trim()
        ));
        Ok(yes)
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
        let web = settings.web_search;
        let request = Request {
            instructions: format!(
                "{}{}",
                request.instructions,
                if web { prompt::WEB } else { prompt::NO_WEB }
            ),
            text: request.text.clone(),
            image: request.image,
        };
        let request = &request;
        match settings.provider {
            Provider::Anthropic => {
                let key = api_key(Provider::Anthropic)?;
                let model = preferred.unwrap_or(anthropic::DEFAULT_MODEL);
                crate::debug(format_args!("Anthropic API: using {model}"));
                anthropic::analyze(&self.http, &key, model, request, sources, progress, web)
            }
            provider => {
                let (token, service) = self.openai_token(provider)?;
                let models = self.models(provider)?;
                let model = openai::choose_model(&models, preferred, service)?;
                crate::debug(format_args!("{provider:?}: using {model} of {models:?}"));
                openai::analyze(
                    &self.http, &token, &model, request, sources, progress, service, web,
                )
            }
        }
    }

    /// The bearer token and service for a provider that uses the Responses API.
    fn openai_token(&mut self, provider: Provider) -> Result<(String, openai::Service)> {
        match provider {
            Provider::ChatGpt => Ok((self.chatgpt.access_token()?, openai::Service::Plan)),
            _ => Ok((api_key(provider)?, openai::Service::ApiKey)),
        }
    }
}

/// Lookups for one request: runs the tool calls the model asks for, within
/// Context's limit, and remembers which sources answered.
struct Lookups<'a> {
    sources: &'a Sources,
    progress: &'a dyn Fn(String),
    calls_left: usize,
    used: Vec<String>,
    links: Vec<Link>,
}

impl<'a> Lookups<'a> {
    fn new(sources: &'a Sources, progress: &'a dyn Fn(String)) -> Self {
        Self {
            sources,
            progress,
            calls_left: prompt::MAX_TOOL_CALLS,
            used: Vec::new(),
            links: Vec::new(),
        }
    }

    /// Notes that the provider searched the web for this answer.
    fn searched(&mut self) {
        const WEB: &str = "the web";
        if !self.used.iter().any(|source| source == WEB) {
            (self.progress)("Searching the web…".into());
            self.used.push(WEB.into());
        }
    }

    /// Keeps a page the answer cites. Only web addresses are kept, since the
    /// user may open them from history.
    fn cite(&mut self, url: &str, title: &str) {
        let url = url.trim();
        let web = url.starts_with("https://") || url.starts_with("http://");
        if !web
            || self.links.len() >= prompt::MAX_LINKS
            || self.links.iter().any(|link| link.url == url)
        {
            return;
        }
        let title: String = title.trim().chars().take(120).collect();
        self.links.push(Link {
            title: if title.is_empty() {
                url.to_owned()
            } else {
                title
            },
            url: url.to_owned(),
        });
    }

    /// The final answer, with what was consulted on the way.
    fn answer(self, text: &str) -> Answer {
        let (text, needs) = prompt::finish(text);
        Answer {
            text,
            sources: self.used,
            links: self.links,
            needs,
        }
    }

    /// Whether the model may still call tools.
    fn left(&self) -> bool {
        self.calls_left > 0
    }

    /// Runs one call. Returns the text for the model and whether it failed.
    fn call(&mut self, name: &str, arguments: &str) -> (String, bool) {
        crate::debug(format_args!("tool call {name} arguments {arguments}"));
        if self.calls_left == 0 {
            return (
                "Context's limit on lookups for one capture was reached.".into(),
                true,
            );
        }
        self.calls_left -= 1;
        if let Some(label) = self.sources.label(name) {
            (self.progress)(format!("Checking {label}…"));
        }
        let output = self.sources.call(name, arguments);
        let failed = output.source.is_none();
        if let Some(source) = output.source
            && !self.used.contains(&source)
        {
            self.used.push(source);
        }
        (output.text, failed)
    }
}

/// A JSON value cut to a readable length for diagnostics.
pub fn short(value: &serde_json::Value) -> String {
    value.to_string().chars().take(2_000).collect()
}

/// A JPEG as base64, the form both APIs take images in.
pub fn base64(jpeg: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(jpeg)
}

/// A JPEG as a `data:` URL.
pub fn data_url(jpeg: &[u8]) -> String {
    format!("data:image/jpeg;base64,{}", base64(jpeg))
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
