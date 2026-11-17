//! The GPUI app: a controller on the UI thread, a worker thread for blocking
//! capture and network calls, and the windows, tray and shortcut around them.

mod card;
mod main_window;
mod native;
mod theme;
mod tray;

use std::{thread, time::Duration};

use async_channel::{Receiver, Sender};
use gpui::{App, Application, WindowHandle};
use uuid::Uuid;

use crate::Result;
use crate::capture::{self, Focus};
use crate::chatgpt::{Answer, ChatGpt, SignInRequired};
use crate::history::{ContextText, History};
use crate::instance::{Instance, Remote};
use crate::sources::{self, SourceStatus, Sources};

use card::Card;
use main_window::MainView;

/// How long to wait after minimizing Context before capturing what is behind it.
const MINIMIZE_DELAY: Duration = Duration::from_millis(450);

/// Something the user asked for, from any surface.
#[derive(Clone)]
pub enum Command {
    Open(Option<Uuid>),
    Capture,
    SignIn,
    SignOut,
    Delete(Uuid),
    ClearHistory,
    /// Opens `mcp.json` in the user's editor.
    EditSources,
    /// Reconnects every source from `mcp.json`.
    ReloadSources,
    Quit,
}

enum Event {
    Command(Command),
    BeginCapture,
    Shortcut(Shortcut),
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
    Sources(SourcesState),
    SignedIn(std::result::Result<String, Problem>),
    SignedOut(String),
}

#[derive(Clone)]
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

#[derive(Clone, PartialEq, Eq)]
pub enum Account {
    Checking,
    SignedOut,
    Connecting,
    SignedIn(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Capturing,
}

/// Connected sources as the main window shows them.
#[derive(Clone)]
pub enum SourcesState {
    Connecting,
    Ready {
        servers: Vec<SourceStatus>,
        /// A problem reading `mcp.json` itself.
        problem: Option<String>,
    },
}

/// How the user triggers a capture from anywhere.
#[derive(Clone)]
pub enum Shortcut {
    Registered(String),
    /// The desktop has no global shortcut API Context can use, so the user binds
    /// `context capture` in their system settings instead.
    External,
    Unavailable,
}

pub fn run(instance: Instance, first: Remote) -> Result<()> {
    let history = History::load();
    let chatgpt = ChatGpt::new()?;
    let sources = Sources::new()?;
    let (events, receive_events) = async_channel::unbounded();
    let (jobs, receive_jobs) = async_channel::unbounded();

    let interrupt = events.clone();
    ctrlc::set_handler(move || {
        let _ = interrupt.try_send(Event::Command(Command::Quit));
    })?;
    let remote = events.clone();
    let _instance_lock = instance.listen(move |command| {
        let command = match command {
            Remote::Open => Command::Open(None),
            Remote::Capture => Command::Capture,
        };
        let _ = remote.try_send(Event::Command(command));
    })?;
    start_worker(chatgpt, sources, receive_jobs, events.clone())?;
    let _ = jobs.try_send(Job::CheckAccount);
    let _ = jobs.try_send(Job::ReloadSources);

    let app = Application::new();
    let reopen = events.clone();
    app.on_reopen(move |_| {
        let _ = reopen.try_send(Event::Command(Command::Open(None)));
    });
    app.run(move |cx| {
        let main = match main_window::open(cx, &history, events.clone()) {
            Ok(main) => main,
            Err(error) => {
                eprintln!("context: could not open the main window. {error}");
                cx.quit();
                return;
            }
        };
        let desktop = tray::Desktop::install(events.clone());
        cx.activate(true);
        if first == Remote::Capture {
            let _ = events.try_send(Event::Command(Command::Capture));
        }
        let mut controller = Controller {
            history,
            main,
            card: None,
            card_opened: false,
            desktop,
            account: Account::Checking,
            activity: Activity::Idle,
            events,
            jobs,
        };
        cx.spawn(async move |app| {
            while let Ok(event) = receive_events.recv().await {
                let keep_going = app
                    .update(|cx| controller.handle(event, cx))
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
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
                        send(Report::Sources(SourcesState::Ready { servers, problem }))
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
    history: History,
    main: WindowHandle<MainView>,
    card: Option<WindowHandle<Card>>,
    /// Whether the current capture has shown its card yet.
    card_opened: bool,
    desktop: tray::Desktop,
    account: Account,
    activity: Activity,
    events: Sender<Event>,
    jobs: Sender<Job>,
}

impl Controller {
    /// Returns false when the app should stop handling events.
    fn handle(&mut self, event: Event, cx: &mut App) -> bool {
        match event {
            Event::Command(Command::Quit) => {
                cx.quit();
                return false;
            }
            Event::Command(Command::Open(selection)) => self.open(selection, cx),
            Event::Command(Command::Capture) => self.request_capture(cx),
            Event::BeginCapture => self.begin_capture(cx),
            Event::Command(Command::SignIn) => self.sign_in(cx),
            Event::Command(Command::SignOut) => {
                if self.submit(Job::SignOut, cx) {
                    self.set_account(Account::Checking, cx);
                }
            }
            Event::Command(Command::Delete(id)) => {
                self.history.remove(id);
                self.refresh_history(None, cx);
            }
            Event::Command(Command::ClearHistory) => {
                self.history.clear();
                self.refresh_history(None, cx);
            }
            Event::Command(Command::EditSources) => {
                let opened = sources::config_path().and_then(|path| Ok(open::that_detached(path)?));
                match opened {
                    Ok(()) => self.notify(
                        "Add servers to mcp.json, save it, then choose Reload.".into(),
                        false,
                        cx,
                    ),
                    Err(error) => {
                        self.notify(format!("Could not open mcp.json. {error}"), true, cx)
                    }
                }
            }
            Event::Command(Command::ReloadSources) => {
                if self.submit(Job::ReloadSources, cx) {
                    self.update_main(cx, |view, cx| {
                        view.set_sources(SourcesState::Connecting, cx)
                    });
                }
            }
            Event::Shortcut(shortcut) => {
                self.desktop.set_shortcut(&shortcut);
                self.update_main(cx, |view, cx| view.set_shortcut(shortcut, cx));
            }
            Event::Worker(report) => self.report(report, cx),
        }
        true
    }

    fn report(&mut self, report: Report, cx: &mut App) {
        match report {
            Report::Account(email) => {
                let account = email.map_or(Account::SignedOut, Account::SignedIn);
                self.set_account(account, cx);
            }
            Report::SignedIn(Ok(email)) => {
                self.set_account(Account::SignedIn(email.clone()), cx);
                self.notify(format!("Connected as {email}."), false, cx);
            }
            Report::SignedIn(Err(problem)) => {
                self.set_account(Account::SignedOut, cx);
                self.notify(problem.message, true, cx);
            }
            Report::SignedOut(message) => {
                self.set_account(Account::SignedOut, cx);
                self.notify(message, false, cx);
            }
            Report::Captured(focus) => {
                self.card_opened = true;
                self.show_card(card::Phase::Thinking, &focus, cx);
            }
            Report::Progress(status) => {
                if let Some(card) = self.card {
                    let _ = card.update(cx, |card, _, cx| card.set_status(status, cx));
                }
            }
            Report::Analysis {
                focus,
                result,
                screenshot,
            } => {
                self.set_activity(Activity::Idle, cx);
                self.finish_analysis(focus, result, screenshot, cx);
            }
            Report::Sources(state) => {
                self.update_main(cx, |view, cx| view.set_sources(state, cx));
            }
        }
    }

    fn finish_analysis(
        &mut self,
        focus: Focus,
        result: std::result::Result<Answer, Problem>,
        screenshot: Option<Vec<u8>>,
        cx: &mut App,
    ) {
        let phase = match result {
            Ok(Answer {
                text: Some(response),
                sources,
            }) => match ContextText::from_response(&response) {
                Some(text) => {
                    let entry = self
                        .history
                        .push(text, focus.app.clone(), sources, screenshot);
                    self.refresh_history(Some(entry.id), cx);
                    card::Phase::Ready(entry)
                }
                None => card::Phase::Empty,
            },
            Ok(Answer { text: None, .. }) => card::Phase::Empty,
            Err(problem) => {
                if problem.sign_in {
                    self.set_account(Account::SignedOut, cx);
                }
                card::Phase::Problem(problem)
            }
        };
        // Replace the card the user is watching with one sized for the result.
        // A card the user dismissed stays closed: results still land in
        // history and problems in the main window.
        let watching = self
            .card
            .is_some_and(|card| card.update(cx, |_, _, _| ()).is_ok());
        if watching {
            self.show_card(phase, &focus, cx);
            return;
        }
        match phase {
            // Nothing was captured, so no card opened yet. Show one now.
            card::Phase::Problem(problem) if !self.card_opened => {
                self.show_card(card::Phase::Problem(problem), &focus, cx);
            }
            card::Phase::Problem(problem) => self.notify(problem.message, true, cx),
            _ => {}
        }
    }

    fn open(&mut self, selection: Option<Uuid>, cx: &mut App) {
        self.close_card(cx);
        let _ = self.main.update(cx, |view, window, cx| {
            if let Some(id) = selection {
                view.select(id, cx);
            }
            window.activate_window();
        });
    }

    fn request_capture(&mut self, cx: &mut App) {
        if self.activity == Activity::Capturing {
            return;
        }
        if self.account == Account::SignedOut {
            let problem = Problem {
                message: "Connect ChatGPT so Context can look at your screen.".into(),
                sign_in: true,
            };
            self.show_card(card::Phase::Problem(problem), &Focus::default(), cx);
            return;
        }
        // Context itself is in front, so step aside and capture what is behind it.
        let main_is_active = cx
            .active_window()
            .is_some_and(|window| window.window_id() == self.main.window_id());
        if main_is_active {
            self.set_activity(Activity::Capturing, cx);
            let _ = self
                .main
                .update(cx, |_, window, _| window.minimize_window());
            let timer = cx.background_executor().timer(MINIMIZE_DELAY);
            let events = self.events.clone();
            cx.spawn(async move |_| {
                timer.await;
                let _ = events.send(Event::BeginCapture).await;
            })
            .detach();
            return;
        }
        self.begin_capture(cx);
    }

    fn begin_capture(&mut self, cx: &mut App) {
        self.close_card(cx);
        self.card_opened = false;
        if self.submit(Job::Analyze, cx) {
            self.set_activity(Activity::Capturing, cx);
        }
    }

    fn sign_in(&mut self, cx: &mut App) {
        if self.account == Account::Connecting {
            return;
        }
        self.close_card(cx);
        if self.submit(Job::SignIn, cx) {
            self.set_account(Account::Connecting, cx);
            self.notify(
                "Finish connecting in your browser. Context will update when you're done.".into(),
                false,
                cx,
            );
        }
    }

    fn submit(&mut self, job: Job, cx: &mut App) -> bool {
        let sent = self.jobs.try_send(job).is_ok();
        if !sent {
            self.notify(
                "Context's background worker stopped. Quit and reopen Context.".into(),
                true,
                cx,
            );
        }
        sent
    }

    fn show_card(&mut self, phase: card::Phase, focus: &Focus, cx: &mut App) {
        self.close_card(cx);
        let app = focus.app.clone();
        match card::open(cx, phase, app, focus.display_id, self.events.clone()) {
            Ok(handle) => self.card = Some(handle),
            Err(error) => self.notify(
                format!("Context could not show its card. {error}"),
                true,
                cx,
            ),
        }
    }

    fn close_card(&mut self, cx: &mut App) {
        if let Some(card) = self.card.take() {
            let _ = card.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn set_account(&mut self, account: Account, cx: &mut App) {
        self.desktop
            .set_signed_in(matches!(account, Account::SignedIn(_)));
        self.account = account.clone();
        self.update_main(cx, |view, cx| view.set_account(account, cx));
    }

    fn set_activity(&mut self, activity: Activity, cx: &mut App) {
        self.activity = activity;
        self.update_main(cx, |view, cx| view.set_activity(activity, cx));
    }

    fn refresh_history(&mut self, select: Option<Uuid>, cx: &mut App) {
        let entries = self.history.entries.clone();
        let problem = self.history.problem;
        self.update_main(cx, |view, cx| {
            view.set_history(entries, problem, select, cx)
        });
    }

    fn notify(&mut self, message: String, error: bool, cx: &mut App) {
        self.update_main(cx, |view, cx| view.set_notice(message, error, cx));
    }

    fn update_main(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut MainView, &mut gpui::Context<MainView>),
    ) {
        let _ = self.main.update(cx, |view, _, cx| f(view, cx));
    }
}
