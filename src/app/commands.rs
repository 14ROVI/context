//! Commands the pages in `ui/` call. Each one either reads published state or
//! hands a request to the controller. Pages only receive text and images as
//! data; they never get file paths, tokens or the ability to run tools.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use tauri::{AppHandle, State, Wry, ipc::Invoke};
use uuid::Uuid;

use super::{CardView, Command, Shared, Snapshot, card};
use crate::history;

type SharedState = Arc<Shared>;

pub fn handlers() -> impl Fn(Invoke<Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        get_state,
        get_card,
        get_screenshot,
        capture,
        sign_in,
        sign_out,
        delete_entry,
        clear_history,
        edit_sources,
        reload_sources,
        dismiss_notice,
        open_entry,
        open_main,
        copy_text,
        card_resize,
        card_close,
        quit,
    ]
}

#[tauri::command]
fn get_state(shared: State<'_, SharedState>) -> Option<Snapshot> {
    shared.snapshot.lock().ok().map(|snapshot| snapshot.clone())
}

#[tauri::command]
fn get_card(shared: State<'_, SharedState>) -> Option<CardView> {
    card::current(&shared)
}

/// The screenshot saved with an entry, as a data URL for an `<img>`.
#[tauri::command]
async fn get_screenshot(shared: State<'_, SharedState>, id: Uuid) -> Result<Option<String>, ()> {
    let entry = shared.snapshot.lock().ok().and_then(|snapshot| {
        snapshot
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
    });
    Ok(entry
        .and_then(|entry| history::screenshot(&entry))
        .map(|jpeg| format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg))))
}

#[tauri::command]
fn capture(shared: State<'_, SharedState>) {
    shared.send(Command::Capture);
}

#[tauri::command]
fn sign_in(shared: State<'_, SharedState>) {
    shared.send(Command::SignIn);
}

#[tauri::command]
fn sign_out(shared: State<'_, SharedState>) {
    shared.send(Command::SignOut);
}

#[tauri::command]
fn delete_entry(shared: State<'_, SharedState>, id: Uuid) {
    shared.send(Command::Delete(id));
}

#[tauri::command]
fn clear_history(shared: State<'_, SharedState>) {
    shared.send(Command::ClearHistory);
}

#[tauri::command]
fn edit_sources(shared: State<'_, SharedState>) {
    shared.send(Command::EditSources);
}

#[tauri::command]
fn reload_sources(shared: State<'_, SharedState>) {
    shared.send(Command::ReloadSources);
}

#[tauri::command]
fn dismiss_notice(shared: State<'_, SharedState>, id: u64) {
    shared.send(Command::DismissNotice(id));
}

#[tauri::command]
fn open_entry(shared: State<'_, SharedState>, id: Uuid) {
    shared.send(Command::Open(Some(id)));
}

#[tauri::command]
fn open_main(shared: State<'_, SharedState>) {
    shared.send(Command::Open(None));
}

/// Copies text without needing page focus, which the card never has.
#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn card_resize(app: AppHandle, shared: State<'_, SharedState>, height: f64) -> Result<(), String> {
    card::fit(&app, &shared, height.clamp(60., 600.)).map_err(|error| error.to_string())
}

#[tauri::command]
fn card_close(app: AppHandle, shared: State<'_, SharedState>) {
    card::close(&app, &shared);
    shared.send(Command::CardClosed);
}

#[tauri::command]
fn quit(shared: State<'_, SharedState>) {
    shared.send(Command::Quit);
}
