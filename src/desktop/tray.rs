//! The tray menu (`tray-icon`) and the global capture shortcut (`global-hotkey`).

use async_channel::Sender;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::{Command, Event, Shortcut};
use crate::Result;

/// Shortcuts to try in order, with the modifiers people expect to read.
const CANDIDATES: [(Modifiers, &str); 2] = [
    (Modifiers::CONTROL.union(Modifiers::ALT), "Ctrl+Alt+Space"),
    (
        Modifiers::CONTROL
            .union(Modifiers::ALT)
            .union(Modifiers::SHIFT),
        "Ctrl+Alt+Shift+Space",
    ),
];

pub struct Desktop {
    _tray: Option<TrayIcon>,
    _hotkeys: Option<GlobalHotKeyManager>,
    menu: Option<TrayMenu>,
}

struct TrayMenu {
    capture: MenuItem,
    sign_in: MenuItem,
    sign_out: MenuItem,
}

impl Desktop {
    pub fn install(events: Sender<Event>) -> Self {
        let (tray, menu) = match build_tray(events.clone()) {
            Ok((tray, menu)) => (Some(tray), Some(menu)),
            Err(error) => {
                eprintln!("context: the tray icon is unavailable. {error}");
                (None, None)
            }
        };
        let (hotkeys, shortcut) = install_shortcut(events.clone());
        let _ = events.try_send(Event::Shortcut(shortcut));
        Self {
            _tray: tray,
            _hotkeys: hotkeys,
            menu,
        }
    }

    pub fn set_signed_in(&self, signed_in: bool) {
        if let Some(menu) = &self.menu {
            menu.sign_in.set_enabled(!signed_in);
            menu.sign_out.set_enabled(signed_in);
        }
    }

    pub fn set_shortcut(&self, shortcut: &Shortcut) {
        if let Some(menu) = &self.menu {
            let text = match shortcut {
                Shortcut::Registered(keys) => format!("Capture screen ({})", display_keys(keys)),
                _ => "Capture screen".into(),
            };
            menu.capture.set_text(text);
        }
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

fn install_shortcut(events: Sender<Event>) -> (Option<GlobalHotKeyManager>, Shortcut) {
    // Wayland compositors do not let apps grab keys. The user binds
    // `context capture` in their desktop settings instead.
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty()) {
        return (None, Shortcut::External);
    }
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => manager,
        Err(error) => {
            eprintln!("context: global shortcuts are unavailable. {error}");
            return (None, Shortcut::Unavailable);
        }
    };
    for (modifiers, label) in CANDIDATES {
        let hotkey = HotKey::new(Some(modifiers), Code::Space);
        if manager.register(hotkey).is_ok() {
            let id = hotkey.id();
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                if event.id == id && event.state == HotKeyState::Pressed {
                    let _ = events.try_send(Event::Command(Command::Capture));
                }
            }));
            return (Some(manager), Shortcut::Registered(label.into()));
        }
    }
    (None, Shortcut::Unavailable)
}

fn build_tray(events: Sender<Event>) -> Result<(TrayIcon, TrayMenu)> {
    let capture = MenuItem::new("Capture screen", true, None);
    let open = MenuItem::new("Open Context", true, None);
    let sign_in = MenuItem::new("Connect ChatGPT…", false, None);
    let sign_out = MenuItem::new("Sign out", false, None);
    let quit = MenuItem::new("Quit Context", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &capture,
        &open,
        &PredefinedMenuItem::separator(),
        &sign_in,
        &sign_out,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    let routes = [
        (capture.id().clone(), Command::Capture),
        (open.id().clone(), Command::Open(None)),
        (sign_in.id().clone(), Command::SignIn),
        (sign_out.id().clone(), Command::SignOut),
        (quit.id().clone(), Command::Quit),
    ];
    let menu_events = events.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some((_, command)) = routes.iter().find(|(id, _)| *id == event.id) {
            let _ = menu_events.try_send(Event::Command(command.clone()));
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = events.try_send(Event::Command(Command::Open(None)));
        }
    }));

    let tray = TrayIconBuilder::new()
        .with_tooltip("Context")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_icon(icon()?)
        .build()?;
    Ok((
        tray,
        TrayMenu {
            capture,
            sign_in,
            sign_out,
        },
    ))
}

/// Draws the Context mark: a rounded square with a cut-out diamond.
fn icon() -> Result<Icon> {
    const SIDE: u32 = 32;
    const ACCENT: [u8; 3] = [0x5b, 0x74, 0xf0];
    let half = SIDE as f32 / 2.0;
    let mut pixels = Vec::with_capacity((SIDE * SIDE * 4) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let dx = (x as f32 + 0.5 - half).abs();
            let dy = (y as f32 + 0.5 - half).abs();
            // Rounded square of radius 7 inside a 1px margin.
            let corner = ((dx - 8.0).max(0.0).powi(2) + (dy - 8.0).max(0.0).powi(2)).sqrt();
            let square = (7.0 - corner + 0.5).clamp(0.0, 1.0);
            let diamond = ((dx + dy) - 5.5).clamp(0.0, 1.0);
            let alpha = (square * diamond * 255.0) as u8;
            pixels.extend_from_slice(&[ACCENT[0], ACCENT[1], ACCENT[2], alpha]);
        }
    }
    Ok(Icon::from_rgba(pixels, SIDE, SIDE)?)
}
