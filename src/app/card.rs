//! The corner card window. It never takes focus and stays above other
//! windows. The page in `ui/card.html` renders the card, measures itself and
//! asks for its size through `card_resize`, so the window always fits the text.

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use super::{Problem, Shared};
use crate::capture::Focus;
use crate::history::Entry;

const LABEL: &str = "card";
pub const WIDTH: f64 = 384.;
/// Space between the card and the edge of the screen's work area.
const MARGIN: f64 = 16.;

#[derive(Clone, Serialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum Phase {
    Thinking,
    Ready { entry: Entry },
    Empty,
    Problem { problem: Problem },
}

/// What the card page renders.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardView {
    #[serde(flatten)]
    phase: Phase,
    app: Option<String>,
    /// What the model is doing right now, while thinking.
    status: Option<String>,
    /// Changes with every new phase, so the page restarts its close timer.
    generation: u64,
    /// The display to open on, as its top-left corner in physical pixels.
    #[serde(skip)]
    origin: Option<(i32, i32)>,
}

/// Shows `phase` in the card, opening the window if needed.
pub fn show(app: &AppHandle, shared: &Shared, phase: Phase, focus: &Focus) {
    let view = {
        let Ok(mut card) = shared.card.lock() else {
            return;
        };
        let previous = card.as_ref();
        let view = CardView {
            phase,
            app: focus
                .app
                .clone()
                .or_else(|| previous.and_then(|view| view.app.clone())),
            status: None,
            generation: previous.map_or(1, |view| view.generation + 1),
            origin: focus
                .display_origin
                .or_else(|| previous.and_then(|view| view.origin)),
        };
        *card = Some(view.clone());
        view
    };
    match app.get_webview_window(LABEL) {
        Some(window) => {
            let _ = window.emit("card", view);
        }
        None => {
            if let Err(error) = open_window(app) {
                eprintln!("context: could not open the card. {error}");
            }
        }
    }
}

pub fn set_status(app: &AppHandle, shared: &Shared, status: String) {
    let view = match shared.card.lock() {
        Ok(mut card) => match card.as_mut() {
            Some(view) => {
                view.status = Some(status);
                view.clone()
            }
            None => return,
        },
        Err(_) => return,
    };
    let _ = app.emit_to(LABEL, "card", view);
}

pub fn close(app: &AppHandle, shared: &Shared) {
    if let Ok(mut card) = shared.card.lock() {
        *card = None;
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.destroy();
    }
}

pub fn is_open(shared: &Shared) -> bool {
    shared.card.lock().is_ok_and(|card| card.is_some())
}

pub fn current(shared: &Shared) -> Option<CardView> {
    shared.card.lock().ok().and_then(|card| card.clone())
}

/// Opens the hidden card window. It shows itself once the page has measured
/// its content and called `card_resize`.
fn open_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("card.html".into()))
        .title("Context")
        .inner_size(WIDTH, 160.)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focusable(false)
        .focused(false)
        .visible(false)
        .build()
}

/// Sizes the card to `height` logical pixels, places it in the corner of its
/// display's work area and shows it.
pub fn fit(app: &AppHandle, shared: &Shared, height: f64) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(LABEL) else {
        return Ok(());
    };
    let origin = shared
        .card
        .lock()
        .ok()
        .and_then(|card| card.as_ref().and_then(|view| view.origin));
    let monitors = app.available_monitors()?;
    let monitor = origin
        .and_then(|(x, y)| {
            monitors
                .iter()
                .find(|monitor| monitor.position().x == x && monitor.position().y == y)
                .cloned()
        })
        .or(app.primary_monitor()?)
        .or_else(|| monitors.first().cloned());
    let Some(monitor) = monitor else {
        return window.show();
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let width = (WIDTH * scale).round() as i32;
    let height = (height * scale).ceil() as i32;
    let margin = (MARGIN * scale).round() as i32;
    let x = area.position.x + area.size.width as i32 - width - margin;
    // macOS shows notifications under the menu bar. Elsewhere the card sits
    // in the bottom corner, above the taskbar.
    let y = if std::env::consts::OS == "macos" {
        area.position.y + margin
    } else {
        area.position.y + area.size.height as i32 - height - margin
    };
    window.set_size(PhysicalSize::new(width as u32, height as u32))?;
    window.set_position(PhysicalPosition::new(x, y))?;
    if !window.is_visible()? {
        window.show()?;
    }
    Ok(())
}
