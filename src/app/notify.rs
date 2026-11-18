//! Native desktop notifications for answers and problems. The system draws
//! them, so they follow the user's notification settings and Do Not Disturb.
//! They carry a title and plain text only. Clicking one opens Context where
//! the user can act on it: the answer's entry, Settings or the usage page.
//! Windows keeps a toast up for its longest built-in time, about 25 seconds;
//! it keeps toasts up until dismissed only when they have a button. Linux
//! keeps them until dismissed. macOS leaves this to the user's alert style for
//! Context in System Settings.

use std::sync::Arc;
use std::thread;

use notify_rust::{Notification, NotificationResponse, Timeout};
use tauri::{AppHandle, Manager};

use super::{Command, FixView, Problem, Shared};
use crate::history::Entry;

/// What a click on a notification opens.
enum Click {
    Command(Command),
    /// An address the app chose, such as a usage page.
    Url(String),
}

/// Shows an answer: its headline, and its summary without Markdown marks.
pub fn answer(app: &AppHandle, entry: &Entry) {
    show(
        app,
        &entry.text.headline,
        &plain(&entry.text.summary),
        Click::Command(Command::Open(Some(entry.id))),
    );
}

/// Shows that there was nothing to add. `needs` names a kind of source that
/// would have helped, such as `calendar`.
pub fn empty(app: &AppHandle, needs: Option<&str>) {
    match needs {
        Some("web") => show(
            app,
            "Context couldn't search the web",
            "This screen needs facts from the web. Turn on web search in Settings.",
            Click::Command(Command::OpenSettings(None)),
        ),
        Some(kind) => show(
            app,
            &format!("Context can't check your {kind} yet"),
            &format!(
                "Connect a {kind} source in Settings and Context can answer questions like this one."
            ),
            Click::Command(Command::OpenSettings(Some(kind.to_owned()))),
        ),
        None => show(
            app,
            "Nothing to add here",
            "Context didn't find anything useful on this screen.",
            Click::Command(Command::Open(None)),
        ),
    }
}

pub fn problem(app: &AppHandle, problem: &Problem) {
    let (title, click) = match &problem.fix {
        Some(FixView::SignIn) => (
            "Connect ChatGPT to continue",
            Click::Command(Command::Open(None)),
        ),
        Some(FixView::Settings) => (
            "Finish setting up Context",
            Click::Command(Command::OpenSettings(None)),
        ),
        Some(FixView::Usage(url)) => ("Usage limit reached", Click::Url(url.clone())),
        Some(FixView::Excluded) => (
            "This app is excluded",
            Click::Command(Command::OpenSettings(None)),
        ),
        None => (
            "Context couldn't help with this one",
            Click::Command(Command::Open(None)),
        ),
    };
    show(app, title, &problem.message, click);
}

/// Says Context is looking at the screen. It is short-lived: Windows and macOS
/// let it time out, and Linux closes it when the check finishes.
/// `automatic` marks a check Context started itself.
pub fn searching(app: &AppHandle, automatic: bool) {
    let body = if automatic {
        "Context noticed something it may help with and is looking into it."
    } else {
        "Context is reading your screen. The answer will appear here."
    };
    let mut notification = base(app, "Searching for context…", body);
    notification.timeout(Timeout::Default);
    #[cfg(all(unix, not(target_os = "macos")))]
    notification.hint(notify_rust::Hint::Transient(true));
    match notification.show() {
        #[cfg(all(unix, not(target_os = "macos")))]
        Ok(handle) => {
            if let Ok(mut searching) = SEARCHING.lock() {
                *searching = Some(handle);
            }
        }
        // Dropping the handle leaves the notification to time out.
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        Ok(_) => {}
        Err(error) => eprintln!("context: could not show a notification. {error}"),
    }
}

/// Closes the searching notification where the system allows it.
pub fn searching_done() {
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(handle) = SEARCHING.lock().ok().and_then(|mut handle| handle.take()) {
        handle.close();
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
static SEARCHING: std::sync::Mutex<Option<notify_rust::NotificationHandle>> =
    std::sync::Mutex::new(None);

/// A notification posted as Context, with plain text only.
fn base(app: &AppHandle, title: &str, body: &str) -> Notification {
    let mut notification = Notification::new();
    notification
        .appname(&app.config().product_name.clone().unwrap_or_default())
        .summary(title)
        .body(body);
    // Windows shows toasts only for a registered app. The installer registers
    // Context; a build from `target` falls back to PowerShell's ID.
    #[cfg(windows)]
    if !unpackaged() {
        notification.app_id(&app.config().identifier);
    }
    // Notification Center needs a bundle to post as. A build from `target`
    // has none, so it posts as Terminal.
    #[cfg(target_os = "macos")]
    {
        static BUNDLE: std::sync::Once = std::sync::Once::new();
        BUNDLE.call_once(|| {
            let bundle = if unpackaged() {
                "com.apple.Terminal".to_owned()
            } else {
                app.config().identifier.clone()
            };
            let _ = notify_rust::set_application(&bundle);
        });
    }
    notification
}

fn show(app: &AppHandle, title: &str, body: &str, click: Click) {
    let mut notification = base(app, title, body);
    // Linux keeps it until dismissed. Windows treats it as its long duration,
    // about 25 seconds. macOS uses the user's alert style.
    notification.timeout(Timeout::Never);
    // Freedesktop servers report a click on the body only for a notification
    // with a `default` action, which they don't draw as a button.
    #[cfg(all(unix, not(target_os = "macos")))]
    notification.action("default", "Open");
    let shared = app.state::<Arc<Shared>>().inner().clone();
    // Each notification waits for its click or close on its own thread, so
    // the controller never blocks on one.
    let waited = thread::Builder::new()
        .name("context-notification".into())
        .spawn(move || {
            let handle = match notification.show() {
                Ok(handle) => handle,
                Err(error) => {
                    eprintln!("context: could not show a notification. {error}");
                    return;
                }
            };
            let waited = handle.wait_for_response(|response: &NotificationResponse| {
                crate::debug(format_args!("notification: {response:?}"));
                if matches!(
                    response,
                    NotificationResponse::Default | NotificationResponse::Action(_)
                ) {
                    match click {
                        Click::Command(command) => shared.send(command),
                        Click::Url(url) => {
                            let _ = open::that_detached(url);
                        }
                    }
                }
            });
            if let Err(error) = waited {
                crate::debug(format_args!("notification: {error}"));
            }
        });
    if let Err(error) = waited {
        eprintln!("context: could not show a notification. {error}");
    }
}

/// Whether this is a build run from Cargo's `target` folder, which Windows
/// and macOS do not know as an app.
#[cfg(any(windows, target_os = "macos"))]
fn unpackaged() -> bool {
    std::env::current_exe().is_ok_and(|exe| {
        exe.parent()
            .and_then(|dir| dir.parent())
            .and_then(|dir| dir.file_name())
            .is_some_and(|name| name == "target")
    })
}

/// Drops inline Markdown marks: bold, italic, code and link addresses.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        // `[label](address)` keeps only the label.
        if c == '['
            && let Some((label, after)) = rest[1..].split_once("](")
            && let Some(end) = after.find(')')
            && !label.contains('\n')
        {
            out.push_str(label);
            rest = &after[end + 1..];
            continue;
        }
        if !matches!(c, '*' | '`') {
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}
