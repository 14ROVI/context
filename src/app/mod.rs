//! The Tauri app. A controller thread owns app state and reacts to events from
//! the windows, tray, shortcut, watcher and worker. The worker thread runs the
//! blocking work. The pages in `ui/` render the state the controller publishes.

mod card;
mod commands;
mod tray;
mod watch;
mod worker;

use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use uuid::Uuid;

use crate::Result;
use crate::capture::Focus;
use crate::history::{ContextText, Entry, History};
use crate::model::{Answer, Assistant, Fix, NeedsAction};
use crate::settings::{Provider, Settings};
use crate::sources::catalog::{self, Entry as CatalogEntry};
use crate::sources::{self, Sources};

pub use card::CardView;
use worker::{Job, Report};

/// How long to wait after hiding Context before capturing what is behind it.
const HIDE_DELAY: Duration = Duration::from_millis(300);
const MAIN: &str = "main";

/// What a launch of `context` asks for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    Open,
    Capture,
}

impl Launch {
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "open" => Some(Self::Open),
            "capture" => Some(Self::Capture),
            _ => None,
        }
    }
}

/// Something the user asked for, from any surface.
#[derive(Clone)]
pub enum Command {
    Open(Option<Uuid>),
    /// Opens Settings, optionally searching sources for a kind such as `calendar`.
    OpenSettings(Option<String>),
    Capture,
    SignIn,
    SignOut,
    Delete(Uuid),
    ClearHistory,
    FollowUp(Uuid, String),
    SaveSettings(Settings),
    SetApiKey(Provider, Option<String>),
    ListModels(Provider),
    EditSources,
    ReloadSources,
    SourceSignIn(String),
    SourceSignOut(String),
    AddSource(
        Box<CatalogEntry>,
        serde_json::Map<String, serde_json::Value>,
    ),
    RemoveSource(String),
    SearchCatalog(String),
    DismissNotice(u64),
    Quit,
}

pub enum Event {
    Command(Command),
    BeginCapture,
    /// The watcher saw new text that may ask the user something.
    AutoCandidate {
        app: String,
        text: String,
    },
    /// The watcher is reading this app, or could not start.
    Watching(std::result::Result<String, String>),
    Shortcut(ShortcutView),
    Worker(Report),
}

/// A failure, with what the user can do about it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub message: String,
    pub fix: Option<FixView>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", content = "url", rename_all = "camelCase")]
pub enum FixView {
    SignIn,
    Settings,
    Usage(String),
    Excluded,
}

impl Problem {
    fn sign_in(&self) -> bool {
        matches!(self.fix, Some(FixView::SignIn))
    }

    pub fn usage_url(&self) -> Option<String> {
        match &self.fix {
            Some(FixView::Usage(url)) => Some(url.clone()),
            _ => None,
        }
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for Problem {
    fn from(error: Box<dyn std::error::Error + Send + Sync>) -> Self {
        let fix = error
            .downcast_ref::<NeedsAction>()
            .map(|needs| match needs.fix {
                Fix::SignIn => FixView::SignIn,
                Fix::Settings => FixView::Settings,
                Fix::Usage(url) => FixView::Usage(url.into()),
                Fix::Excluded => FixView::Excluded,
            });
        Self {
            message: error.to_string(),
            fix,
        }
    }
}

/// Everything the main window shows. Published whole on every change.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    account: Account,
    capturing: bool,
    shortcut: ShortcutView,
    sources: SourcesView,
    notice: Option<Notice>,
    history_problem: Option<String>,
    entries: Vec<Entry>,
    settings: Settings,
    /// Which providers have an API key saved. The keys themselves never leave Rust.
    api_keys: ApiKeys,
    models: ModelsView,
    /// The entry whose follow-up question is being answered.
    asking: Option<Uuid>,
    catalog: CatalogView,
    /// The app automatic cards are watching.
    watching: Option<String>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogView {
    featured: Vec<CatalogEntry>,
    query: String,
    searching: bool,
    results: Option<Vec<CatalogEntry>>,
    error: Option<String>,
    /// A source action, such as a sign-in, is running.
    busy: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "email", rename_all = "camelCase")]
pub enum Account {
    Checking,
    SignedOut,
    Connecting,
    SignedIn(String),
}

/// How the user triggers a capture from anywhere.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", content = "keys", rename_all = "camelCase")]
pub enum ShortcutView {
    Pending,
    Registered(String),
    /// The desktop has no global shortcut API Context can use, so the user
    /// binds `context capture` in their system settings instead.
    External,
    Unavailable,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourcesView {
    connecting: bool,
    servers: Vec<SourceView>,
    problem: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceView {
    name: String,
    enabled: usize,
    hidden: usize,
    problem: Option<String>,
    needs_sign_in: bool,
    signed_in: bool,
    builtin: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Notice {
    id: u64,
    message: String,
    error: bool,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeys {
    open_ai: bool,
    anthropic: bool,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelsView {
    provider: Provider,
    loading: bool,
    list: Vec<String>,
    error: Option<String>,
}

/// State the pages can read at any time through commands.
pub struct Shared {
    events: Sender<Event>,
    snapshot: Mutex<Snapshot>,
    card: Mutex<Option<CardView>>,
}

impl Shared {
    fn send(&self, command: Command) {
        let _ = self.events.try_send(Event::Command(command));
    }
}

pub fn run(first: Launch) -> Result<()> {
    let settings = Settings::load();
    let mut history = History::load();
    history.prune(settings.history_days);
    let assistant = Assistant::new()?;
    let sources = Sources::new()?;
    let (events, receive_events) = async_channel::unbounded();
    let (jobs, receive_jobs) = async_channel::unbounded();
    worker::start(
        assistant,
        sources,
        settings.clone(),
        receive_jobs,
        events.clone(),
    )?;
    let _ = jobs.try_send(Job::CheckAccount);
    let _ = jobs.try_send(Job::ReloadSources);
    let watched = Arc::new(RwLock::new(settings.clone()));

    let snapshot = Snapshot {
        account: Account::Checking,
        capturing: false,
        shortcut: ShortcutView::Pending,
        sources: SourcesView {
            connecting: true,
            servers: Vec::new(),
            problem: None,
        },
        notice: None,
        history_problem: history.problem.map(str::to_owned),
        entries: history.entries.clone(),
        api_keys: api_keys(),
        models: ModelsView {
            provider: settings.provider,
            ..Default::default()
        },
        settings: settings.clone(),
        asking: None,
        catalog: CatalogView {
            featured: catalog::featured(),
            ..Default::default()
        },
        watching: None,
    };
    let shared = Arc::new(Shared {
        events: events.clone(),
        snapshot: Mutex::new(snapshot),
        card: Mutex::new(None),
    });

    let relaunch = events.clone();
    let shortcut_events = events.clone();
    tauri::Builder::default()
        // Must be first: a second `context` process hands its command to this
        // one and exits.
        .plugin(tauri_plugin_single_instance::init(move |_, argv, _| {
            let command = match argv.get(1).and_then(|word| Launch::parse(word)) {
                Some(Launch::Capture) => Command::Capture,
                _ => Command::Open(None),
            };
            let _ = relaunch.try_send(Event::Command(command));
        }))
        .plugin(tray::shortcut_plugin(shortcut_events))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(shared.clone())
        .invoke_handler(commands::handlers())
        .setup(move |app| {
            let handle = app.handle().clone();
            open_main_window(&handle)?;
            let models = crate::ocr::model_dir(handle.path().resource_dir().ok());
            watch::start(watched.clone(), models, events.clone());
            let menu = tray::install(&handle, events.clone())?;
            let shortcut = tray::register_shortcut(&handle, &settings.shortcut);
            let _ = events.try_send(Event::Shortcut(shortcut));
            if first == Launch::Capture {
                let _ = events.try_send(Event::Command(Command::Capture));
            }
            let controller = Controller {
                app: handle,
                shared,
                history,
                menu,
                settings,
                watched,
                account: Account::Checking,
                capturing: false,
                card_opened: false,
                notice_id: 0,
                events,
                jobs,
            };
            thread::Builder::new()
                .name("context-controller".into())
                .spawn(move || controller.run(receive_events))?;
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}

fn open_main_window(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Context")
        .inner_size(980., 680.)
        .min_inner_size(720., 480.)
        .center()
        .build()?;
    // Closing the window hides it. Context keeps running in the tray.
    let hide = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = hide.hide();
        }
    });
    Ok(())
}

fn api_keys() -> ApiKeys {
    ApiKeys {
        open_ai: Assistant::has_api_key(Provider::OpenAi),
        anthropic: Assistant::has_api_key(Provider::Anthropic),
    }
}

struct Controller {
    app: AppHandle,
    shared: Arc<Shared>,
    history: History,
    menu: tray::Menu,
    settings: Settings,
    /// The settings the watcher reads.
    watched: Arc<RwLock<Settings>>,
    account: Account,
    capturing: bool,
    /// Whether the current capture has shown its card yet.
    card_opened: bool,
    notice_id: u64,
    events: Sender<Event>,
    jobs: Sender<Job>,
}

impl Controller {
    fn run(mut self, events: Receiver<Event>) {
        while let Ok(event) = events.recv_blocking() {
            if !self.handle(event) {
                break;
            }
        }
    }

    /// Returns false when the app should stop.
    fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::Command(Command::Quit) => {
                self.app.exit(0);
                return false;
            }
            Event::Command(command) => self.command(command),
            Event::BeginCapture => self.begin_capture(),
            Event::AutoCandidate { app, text } => self.auto_check(&app, text),
            Event::Watching(Ok(app)) => self.update(|snapshot| snapshot.watching = Some(app)),
            Event::Watching(Err(message)) => {
                self.notify(message, true);
                let mut next = self.settings.clone();
                next.auto_cards = false;
                self.save_settings(next);
            }
            Event::Shortcut(shortcut) => {
                self.menu.set_shortcut(&shortcut);
                self.update(|snapshot| snapshot.shortcut = shortcut);
            }
            Event::Worker(report) => self.report(report),
        }
        true
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Quit => {}
            Command::Open(selection) => self.open(selection),
            Command::OpenSettings(search) => {
                self.open(None);
                let _ = self.app.emit_to(MAIN, "open-settings", search);
            }
            Command::Capture => self.request_capture(),
            Command::SignIn => self.sign_in(),
            Command::SignOut => {
                if self.submit(Job::SignOut) {
                    self.account = Account::Checking;
                    self.publish();
                }
            }
            Command::Delete(id) => {
                self.history.remove(id);
                self.publish();
            }
            Command::ClearHistory => {
                self.history.clear();
                self.publish();
            }
            Command::FollowUp(id, question) => self.follow_up(id, question),
            Command::SaveSettings(settings) => self.save_settings(settings),
            Command::SetApiKey(provider, key) => {
                self.submit(Job::SetApiKey(provider, key));
            }
            Command::ListModels(provider) => {
                if self.submit(Job::ListModels(provider)) {
                    self.update(|snapshot| {
                        snapshot.models = ModelsView {
                            provider,
                            loading: true,
                            ..Default::default()
                        };
                    });
                }
            }
            Command::EditSources => {
                match sources::config_path().and_then(|path| Ok(open::that_detached(path)?)) {
                    Ok(()) => self.notify(
                        "Add servers to mcp.json, save it, then choose Reload.".into(),
                        false,
                    ),
                    Err(error) => self.notify(format!("Could not open mcp.json. {error}"), true),
                }
            }
            Command::ReloadSources => {
                if self.submit(Job::ReloadSources) {
                    self.update(|snapshot| snapshot.sources.connecting = true);
                }
            }
            Command::SourceSignIn(name) => {
                if self.submit(Job::SourceSignIn(name.clone())) {
                    self.notify(
                        format!("Finish signing in to {name} in your browser."),
                        false,
                    );
                    self.update(|snapshot| snapshot.catalog.busy = true);
                }
            }
            Command::SourceSignOut(name) => {
                self.submit(Job::SourceSignOut(name));
            }
            Command::AddSource(entry, values) => {
                if self.submit(Job::AddSource(entry.clone(), values)) {
                    if entry.sign_in {
                        self.notify(
                            format!("Finish signing in to {} in your browser.", entry.name),
                            false,
                        );
                    }
                    self.update(|snapshot| snapshot.catalog.busy = true);
                }
            }
            Command::RemoveSource(name) => match sources::connector_id(&name) {
                Some(id) => {
                    self.submit(Job::SourceSignOut(name));
                    let mut next = self.settings.clone();
                    next.connectors.retain(|connector| connector != id);
                    self.save_settings(next);
                    self.submit(Job::ReloadSources);
                }
                None => {
                    self.submit(Job::RemoveSource(name));
                }
            },
            Command::SearchCatalog(query) => {
                let query = query.trim().to_owned();
                if query.is_empty() {
                    self.update(|snapshot| {
                        snapshot.catalog.query.clear();
                        snapshot.catalog.results = None;
                        snapshot.catalog.error = None;
                    });
                } else if self.submit(Job::SearchCatalog(query.clone())) {
                    self.update(|snapshot| {
                        snapshot.catalog.query = query;
                        snapshot.catalog.searching = true;
                        snapshot.catalog.error = None;
                    });
                }
            }
            Command::DismissNotice(id) => {
                self.update(|snapshot| {
                    if snapshot
                        .notice
                        .as_ref()
                        .is_some_and(|notice| notice.id == id)
                    {
                        snapshot.notice = None;
                    }
                });
            }
        }
    }

    fn report(&mut self, report: Report) {
        match report {
            Report::Account(email) => {
                self.account = email.map_or(Account::SignedOut, Account::SignedIn);
                self.publish();
            }
            Report::SignedIn(Ok(email)) => {
                self.account = Account::SignedIn(email.clone());
                self.notify(format!("Connected as {email}."), false);
            }
            Report::SignedIn(Err(problem)) => {
                self.account = Account::SignedOut;
                self.notify(problem.message, true);
            }
            Report::SignedOut(message) => {
                self.account = Account::SignedOut;
                self.notify(message, false);
            }
            Report::Captured(focus) => {
                self.card_opened = true;
                card::show(&self.app, &self.shared, card::Phase::Thinking, &focus);
            }
            Report::Progress(status) => card::set_status(&self.app, &self.shared, status),
            Report::Analysis {
                focus,
                quiet,
                result,
                screenshot,
            } => {
                if !quiet {
                    self.capturing = false;
                }
                self.finish_analysis(focus, quiet, result, screenshot);
                self.publish();
            }
            Report::FollowUp { id, result } => {
                match result {
                    Ok(follow_up) => self.history.add_follow_up(id, follow_up),
                    Err(problem) => {
                        if problem.sign_in() {
                            self.account = Account::SignedOut;
                        }
                        self.notify(problem.message, true);
                    }
                }
                self.update(|snapshot| snapshot.asking = None);
            }
            Report::Sources(servers, problem) => self.update(|snapshot| {
                snapshot.sources = SourcesView {
                    connecting: false,
                    servers: servers
                        .into_iter()
                        .map(|server| SourceView {
                            name: server.name,
                            enabled: server.enabled,
                            hidden: server.hidden,
                            problem: server.problem,
                            needs_sign_in: server.needs_sign_in,
                            signed_in: server.signed_in,
                            builtin: server.builtin,
                        })
                        .collect(),
                    problem,
                };
            }),
            Report::SourceDone(result) => {
                match result {
                    Ok(message) => self.notify(message, false),
                    Err(message) => self.notify(message, true),
                }
                self.update(|snapshot| snapshot.catalog.busy = false);
            }
            Report::ConnectorAdded(id) => {
                if !self.settings.connectors.contains(&id) {
                    let mut next = self.settings.clone();
                    next.connectors.push(id.clone());
                    self.save_settings(next);
                }
                self.submit(Job::ReloadSources);
                if let Some(name) = sources::connector_name(&id) {
                    self.notify(
                        format!("Finish signing in to {name} in your browser."),
                        false,
                    );
                    self.submit(Job::SourceSignIn(name.into()));
                }
            }
            Report::Catalog(query, result) => self.update(|snapshot| {
                if snapshot.catalog.query != query {
                    return;
                }
                snapshot.catalog.searching = false;
                match result {
                    Ok(results) => snapshot.catalog.results = Some(results),
                    Err(error) => snapshot.catalog.error = Some(error),
                }
            }),
            Report::ApiKeySaved(provider, result) => {
                match result {
                    Ok(()) => {
                        self.notify("API key saved.".into(), false);
                        self.submit(Job::ListModels(provider));
                    }
                    Err(error) => self.notify(format!("Could not save the API key. {error}"), true),
                }
                self.update(|snapshot| snapshot.api_keys = api_keys());
            }
            Report::Models(provider, result) => self.update(|snapshot| {
                let (list, error) = match result {
                    Ok(list) => (list, None),
                    Err(error) => (Vec::new(), Some(error)),
                };
                snapshot.models = ModelsView {
                    provider,
                    loading: false,
                    list,
                    error,
                };
            }),
        }
    }

    fn finish_analysis(
        &mut self,
        focus: Focus,
        quiet: bool,
        result: std::result::Result<Answer, Problem>,
        screenshot: Option<Vec<u8>>,
    ) {
        let screenshot = screenshot.filter(|_| self.settings.save_screenshots);
        let phase = match result {
            Ok(Answer {
                text: Some(response),
                sources,
                needs,
            }) => match ContextText::from_response(&response) {
                Some(text) => {
                    let entry = self
                        .history
                        .push(text, focus.app.clone(), sources, screenshot);
                    self.history.prune(self.settings.history_days);
                    card::Phase::Ready { entry, needs }
                }
                None => card::Phase::Empty { needs },
            },
            Ok(Answer {
                text: None, needs, ..
            }) => card::Phase::Empty { needs },
            Err(problem) => {
                if problem.sign_in() {
                    self.account = Account::SignedOut;
                }
                card::Phase::Problem { problem }
            }
        };
        if quiet {
            // Automatic checks only speak up when they found something.
            match phase {
                card::Phase::Ready { .. } if !card::is_open(&self.shared) => {
                    card::show(&self.app, &self.shared, phase, &focus);
                }
                card::Phase::Problem { problem } => {
                    crate::debug(format_args!("automatic check failed: {}", problem.message));
                }
                _ => {}
            }
            return;
        }
        // Update the card the user is watching, or show one if the capture
        // failed before a card opened. A card the user dismissed stays closed:
        // results still land in history and problems in the main window.
        if card::is_open(&self.shared) || !self.card_opened {
            card::show(&self.app, &self.shared, phase, &focus);
        } else if let card::Phase::Problem { problem } = phase {
            self.notify(problem.message, true);
        }
    }

    fn open(&mut self, selection: Option<Uuid>) {
        card::close(&self.app, &self.shared);
        if let Some(window) = self.app.get_webview_window(MAIN) {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
            if let Some(id) = selection {
                let _ = window.emit("select-entry", id);
            }
        }
    }

    fn request_capture(&mut self) {
        if self.capturing {
            return;
        }
        if self.settings.provider == Provider::ChatGpt && self.account == Account::SignedOut {
            let problem = Problem {
                message: "Connect ChatGPT so Context can look at your screen.".into(),
                fix: Some(FixView::SignIn),
            };
            card::show(
                &self.app,
                &self.shared,
                card::Phase::Problem { problem },
                &Focus::default(),
            );
            return;
        }
        // Context itself is in front, so step aside and capture what is behind it.
        let main = self.app.get_webview_window(MAIN);
        if main
            .as_ref()
            .is_some_and(|window| window.is_focused().unwrap_or(false))
        {
            self.capturing = true;
            self.publish();
            if let Some(window) = main {
                let _ = window.hide();
            }
            let events = self.events.clone();
            thread::spawn(move || {
                thread::sleep(HIDE_DELAY);
                let _ = events.send_blocking(Event::BeginCapture);
            });
            return;
        }
        self.begin_capture();
    }

    fn begin_capture(&mut self) {
        card::close(&self.app, &self.shared);
        self.card_opened = false;
        if self.submit(Job::Analyze) {
            self.capturing = true;
            self.publish();
        }
    }

    /// Passes new on-screen text to the worker's gate when automatic cards are
    /// on and nothing else is going on.
    fn auto_check(&mut self, app: &str, text: String) {
        let ready = match self.settings.provider {
            Provider::ChatGpt => matches!(self.account, Account::SignedIn(_)),
            _ => true,
        };
        if !self.settings.auto_cards
            || !ready
            || self.capturing
            || card::is_open(&self.shared)
            || self.settings.excludes(app)
        {
            return;
        }
        self.submit(Job::AutoCandidate(text));
    }

    fn follow_up(&mut self, id: Uuid, question: String) {
        let question = question.trim().to_owned();
        let Some(entry) = self.history.get(id).cloned() else {
            return;
        };
        if question.is_empty() {
            return;
        }
        if self.submit(Job::FollowUp { entry, question }) {
            self.update(|snapshot| snapshot.asking = Some(id));
        }
    }

    fn save_settings(&mut self, mut next: Settings) {
        next.shortcut = next.shortcut.trim().to_owned();
        if next.shortcut != self.settings.shortcut {
            let shortcut = tray::register_shortcut(&self.app, &next.shortcut);
            match &shortcut {
                ShortcutView::Registered(keys) if *keys != next.shortcut => {
                    self.notify(
                        format!(
                            "{} is taken or not valid, so Context uses {keys}.",
                            next.shortcut
                        ),
                        true,
                    );
                    next.shortcut = keys.clone();
                }
                _ => {}
            }
            self.menu.set_shortcut(&shortcut);
            self.update(|snapshot| snapshot.shortcut = shortcut);
        }
        if next.start_at_login != self.settings.start_at_login {
            let autostart = self.app.autolaunch();
            let result = if next.start_at_login {
                autostart.enable()
            } else {
                autostart.disable()
            };
            if let Err(error) = result {
                self.notify(format!("Could not change start at login. {error}"), true);
                next.start_at_login = self.settings.start_at_login;
            }
        }
        if let Ok(mut watched) = self.watched.write() {
            *watched = next.clone();
        }
        if !next.auto_cards {
            self.update(|snapshot| snapshot.watching = None);
        }
        if next.history_days != self.settings.history_days {
            self.history.prune(next.history_days);
        }
        let provider_changed = next.provider != self.settings.provider;
        if let Err(error) = next.save() {
            self.notify(format!("Could not save settings. {error}"), true);
        }
        self.submit(Job::Configure(next.clone()));
        self.settings = next.clone();
        self.update(|snapshot| snapshot.settings = next);
        if provider_changed {
            self.command(Command::ListModels(self.settings.provider));
        }
    }

    fn sign_in(&mut self) {
        if self.account == Account::Connecting {
            return;
        }
        card::close(&self.app, &self.shared);
        if self.submit(Job::SignIn) {
            self.account = Account::Connecting;
            self.notify(
                "Finish connecting in your browser. Context will update when you're done.".into(),
                false,
            );
        }
    }

    fn submit(&mut self, job: Job) -> bool {
        let sent = self.jobs.try_send(job).is_ok();
        if !sent {
            self.notify(
                "Context's background worker stopped. Quit and reopen Context.".into(),
                true,
            );
        }
        sent
    }

    fn notify(&mut self, message: String, error: bool) {
        self.notice_id += 1;
        let id = self.notice_id;
        self.update(|snapshot| {
            snapshot.notice = Some(Notice { id, message, error });
        });
    }

    /// Changes the published state, then sends it to the main window.
    fn update(&mut self, change: impl FnOnce(&mut Snapshot)) {
        if let Ok(mut snapshot) = self.shared.snapshot.lock() {
            change(&mut snapshot);
        }
        self.publish();
    }

    fn publish(&mut self) {
        self.menu
            .set_signed_in(matches!(self.account, Account::SignedIn(_)));
        let snapshot = match self.shared.snapshot.lock() {
            Ok(mut snapshot) => {
                snapshot.account = self.account.clone();
                snapshot.capturing = self.capturing;
                snapshot.entries.clone_from(&self.history.entries);
                snapshot.history_problem = self.history.problem.map(str::to_owned);
                snapshot.clone()
            }
            Err(_) => return,
        };
        let _ = self.app.emit_to(MAIN, "state", snapshot);
    }
}
