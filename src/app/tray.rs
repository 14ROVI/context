//! The tray menu and the global capture shortcut.

use async_channel::Sender;
use tauri::menu::{Menu as TauriMenu, MenuItem, PredefinedMenuItem};
use tauri::plugin::TauriPlugin;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Wry};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use super::{Command, Event, ShortcutView};

/// Shortcuts to try in order, with the label people expect to read.
const CANDIDATES: [(Modifiers, &str); 2] = [
    (Modifiers::CONTROL.union(Modifiers::ALT), "Ctrl+Alt+Space"),
    (
        Modifiers::CONTROL
            .union(Modifiers::ALT)
            .union(Modifiers::SHIFT),
        "Ctrl+Alt+Shift+Space",
    ),
];

/// Tray menu items whose text or state changes.
pub struct Menu {
    capture: MenuItem<Wry>,
    sign_in: MenuItem<Wry>,
    sign_out: MenuItem<Wry>,
}

impl Menu {
    pub fn set_signed_in(&self, signed_in: bool) {
        let _ = self.sign_in.set_enabled(!signed_in);
        let _ = self.sign_out.set_enabled(signed_in);
    }

    pub fn set_shortcut(&self, shortcut: &ShortcutView) {
        let text = match shortcut {
            ShortcutView::Registered(keys) => format!("Capture screen ({})", display_keys(keys)),
            _ => "Capture screen".into(),
        };
        let _ = self.capture.set_text(text);
    }
}

/// Writes a shortcut the way the current platform labels keys.
pub fn display_keys(keys: &str) -> String {
    if std::env::consts::OS == "macos" {
        keys.replace("Ctrl+", "⌃")
            .replace("Alt+", "⌥")
            .replace("Shift+", "⇧")
    } else {
        keys.to_owned()
    }
}

pub fn install(app: &AppHandle, events: Sender<Event>) -> tauri::Result<Menu> {
    let capture = MenuItem::with_id(app, "capture", "Capture screen", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Context", true, None::<&str>)?;
    let sign_in = MenuItem::with_id(app, "sign-in", "Connect ChatGPT…", false, None::<&str>)?;
    let sign_out = MenuItem::with_id(app, "sign-out", "Sign out", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Context", true, None::<&str>)?;
    let menu = TauriMenu::with_items(
        app,
        &[
            &capture,
            &open,
            &PredefinedMenuItem::separator(app)?,
            &sign_in,
            &sign_out,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let menu_events = events.clone();
    let mut tray = TrayIconBuilder::with_id("context")
        .tooltip("Context")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |_, event| {
            let command = match event.id().as_ref() {
                "capture" => Command::Capture,
                "open" => Command::Open(None),
                "sign-in" => Command::SignIn,
                "sign-out" => Command::SignOut,
                "quit" => Command::Quit,
                _ => return,
            };
            let _ = menu_events.try_send(Event::Command(command));
        })
        .on_tray_icon_event(move |_, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = events.try_send(Event::Command(Command::Open(None)));
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(Menu {
        capture,
        sign_in,
        sign_out,
    })
}

/// The plugin that turns a pressed global shortcut into a capture.
pub fn shortcut_plugin(events: Sender<Event>) -> TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(move |_, _, event| {
            if event.state == ShortcutState::Pressed {
                let _ = events.try_send(Event::Command(Command::Capture));
            }
        })
        .build()
}

/// Registers `preferred`, such as `Ctrl+Alt+Space`, replacing any earlier
/// shortcut. If it is taken or not valid, falls back to the defaults.
pub fn register_shortcut(app: &AppHandle, preferred: &str) -> ShortcutView {
    // Wayland compositors do not let apps grab keys. The user binds
    // `context capture` in their desktop settings instead.
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty()) {
        return ShortcutView::External;
    }
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    if !preferred.is_empty() && shortcuts.register(preferred).is_ok() {
        return ShortcutView::Registered(preferred.to_owned());
    }
    for (modifiers, label) in CANDIDATES {
        let shortcut = Shortcut::new(Some(modifiers), Code::Space);
        if app.global_shortcut().register(shortcut).is_ok() {
            return ShortcutView::Registered(label.into());
        }
    }
    ShortcutView::Unavailable
}
