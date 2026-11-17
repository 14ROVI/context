//! The Tauri app. A controller thread owns app state and reacts to events from
//! the windows, tray, shortcut and worker. The worker thread runs the blocking
//! work: capture, model requests, sign-in and connecting sources. The pages in
//! `ui/` render the state the controller publishes.

mod card;
mod commands;
mod tray;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use uuid::Uuid;

use crate::Result;
use crate::capture::{self, Focus};
use crate::chatgpt::{Answer, ChatGpt, SignInRequired};
use crate::history::{ContextText, Entry, History};
use crate::sources::{self, SourceStatus, Sources};

pub use card::CardView;

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
    Capture,
    SignIn,
    SignOut,
    Delete(Uuid),
    ClearHistory,
    EditSources,
    ReloadSources,
    DismissNotice(u64),
    CardClosed,
    Quit,
}

enum Event {
    Command(Command),
    BeginCapture,
    Shortcut(ShortcutView),
    Worker(Report),
}

/// Blocking work for the worker thread.
enum Job {
    CheckAccount,
    Analyze,
    SignIn,
    SignOut,
    ReloadSources,
}

/// What the worker reports back.
enum Report {
    Account(Option<String>),
    Captured(Focus),
    /// A short status for the card while the model works, such as "Checking calendar…".
    Progress(String),
    Analysis {
        focus: Focus,
        result: std::result::Result<Answer, Problem>,
        /// The JPEG that was analyzed, kept with a saved result.
        screenshot: Option<Vec<u8>>,
    },
    SignedIn(std::result::Result<String, Problem>),
    SignedOut(String),
    Sources(Vec<SourceStatus>, Option<String>),
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub message: String,
    /// True when connecting ChatGPT again would fix it.
    pub sign_in: bool,
}

impl From<Box<dyn std::error::Error + Send + Sync>> for Problem {
    fn from(error: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self {
            sign_in: error.downcast_ref::<SignInRequired>().is_some(),
            message: error.to_string(),
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
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Notice {
    id: u64,
    message: String,
    error: bool,
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
    let history = History::load();
    let chatgpt = ChatGpt::new()?;
    let sources = Sources::new()?;
    let (events, receive_events) = async_channel::unbounded();
    let (jobs, receive_jobs) = async_channel::unbounded();
    start_worker(chatgpt, sources, receive_jobs, events.clone())?;
    let _ = jobs.try_send(Job::CheckAccount);
    let _ = jobs.try_send(Job::ReloadSources);

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
        .manage(shared.clone())
        .invoke_handler(commands::handlers())
        .setup(move |app| {
            let handle = app.handle().clone();
            open_main_window(&handle)?;
            let menu = tray::install(&handle, events.clone())?;
            let shortcut = tray::register_shortcut(&handle);
            let _ = events.try_send(Event::Shortcut(shortcut));
            if first == Launch::Capture {
                let _ = events.try_send(Event::Command(Command::Capture));
            }
            let controller = Controller {
                app: handle,
                shared,
                history,
                menu,
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
        .inner_size(980., 640.)
        .min_inner_size(720., 460.)
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

fn start_worker(
    mut chatgpt: ChatGpt,
    mut sources: Sources,
    jobs: Receiver<Job>,
    events: Sender<Event>,
) -> Result<()> {
    thread::Builder::new()
        .name("context-worker".into())
        .spawn(move || {
            let send = |report| events.send_blocking(Event::Worker(report)).is_ok();
            while let Ok(job) = jobs.recv_blocking() {
                let delivered = match job {
                    Job::CheckAccount => send(Report::Account(chatgpt.email().ok().flatten())),
                    Job::SignIn => send(Report::SignedIn(chatgpt.sign_in().map_err(Problem::from))),
                    Job::SignOut => send(Report::SignedOut(
                        chatgpt.sign_out().unwrap_or_else(|error| error.to_string()),
                    )),
                    Job::Analyze => analyze(&mut chatgpt, &sources, &send),
                    Job::ReloadSources => {
                        sources.reload();
                        let (servers, problem) = sources.status();
                        send(Report::Sources(servers, problem))
                    }
                };
                if !delivered {
                    break;
                }
            }
        })?;
    Ok(())
}

fn analyze(chatgpt: &mut ChatGpt, sources: &Sources, send: &impl Fn(Report) -> bool) -> bool {
    let failed = |focus, error: Box<dyn std::error::Error + Send + Sync>| Report::Analysis {
        focus,
        result: Err(error.into()),
        screenshot: None,
    };
    if let Err(error) = chatgpt.ensure_connected() {
        return send(failed(Focus::default(), error));
    }
    let snapshot = match capture::take() {
        Ok(snapshot) => snapshot,
        Err(error) => return send(failed(Focus::default(), error)),
    };
    let focus = snapshot.focus.clone();
    if !send(Report::Captured(focus.clone())) {
        return false;
    }
    let progress = |status: String| {
        send(Report::Progress(status));
    };
    let result = chatgpt
        .analyze(&snapshot, sources, &progress)
        .map_err(Problem::from);
    send(Report::Analysis {
        focus,
        result,
        screenshot: Some(snapshot.jpeg),
    })
}

struct Controller {
    app: AppHandle,
    shared: Arc<Shared>,
    history: History,
    menu: tray::Menu,
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
            Event::Command(Command::Open(selection)) => self.open(selection),
            Event::Command(Command::Capture) => self.request_capture(),
            Event::BeginCapture => self.begin_capture(),
            Event::Command(Command::SignIn) => self.sign_in(),
            Event::Command(Command::SignOut) => {
                if self.submit(Job::SignOut) {
                    self.account = Account::Checking;
                    self.publish();
                }
            }
            Event::Command(Command::Delete(id)) => {
                self.history.remove(id);
                self.publish();
            }
            Event::Command(Command::ClearHistory) => {
                self.history.clear();
                self.publish();
            }
            Event::Command(Command::EditSources) => {
                match sources::config_path().and_then(|path| Ok(open::that_detached(path)?)) {
                    Ok(()) => self.notify(
                        "Add servers to mcp.json, save it, then choose Reload.".into(),
                        false,
                    ),
                    Err(error) => self.notify(format!("Could not open mcp.json. {error}"), true),
                }
            }
            Event::Command(Command::ReloadSources) => {
                if self.submit(Job::ReloadSources) {
                    self.update(|snapshot| snapshot.sources.connecting = true);
                }
            }
            Event::Command(Command::DismissNotice(id)) => {
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
            Event::Command(Command::CardClosed) => {}
            Event::Shortcut(shortcut) => {
                self.menu.set_shortcut(&shortcut);
                self.update(|snapshot| snapshot.shortcut = shortcut);
            }
            Event::Worker(report) => self.report(report),
        }
        true
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
                result,
                screenshot,
            } => {
                self.capturing = false;
                self.finish_analysis(focus, result, screenshot);
                self.publish();
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
                        })
                        .collect(),
                    problem,
                };
            }),
        }
    }

    fn finish_analysis(
        &mut self,
        focus: Focus,
        result: std::result::Result<Answer, Problem>,
        screenshot: Option<Vec<u8>>,
    ) {
        let phase = match result {
            Ok(Answer {
                text: Some(response),
                sources,
            }) => match ContextText::from_response(&response) {
                Some(text) => card::Phase::Ready {
                    entry: self
                        .history
                        .push(text, focus.app.clone(), sources, screenshot),
                },
                None => card::Phase::Empty,
            },
            Ok(Answer { text: None, .. }) => card::Phase::Empty,
            Err(problem) => {
                if problem.sign_in {
                    self.account = Account::SignedOut;
                }
                card::Phase::Problem { problem }
            }
        };
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
        if self.account == Account::SignedOut {
            let problem = Problem {
                message: "Connect ChatGPT so Context can look at your screen.".into(),
                sign_in: true,
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
