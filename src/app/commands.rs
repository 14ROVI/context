//! Commands the pages in `ui/` call. Each one either reads published state or
//! hands a request to the controller. Pages only receive text and images as
//! data; they never get file paths, tokens or the ability to run tools.

use std::sync::Arc;

use tauri::{AppHandle, State, Wry, ipc::Invoke};
use uuid::Uuid;

use super::{CardView, Command, Shared, Snapshot, card};
use crate::history;
use crate::settings::{Provider, Settings};

type SharedState = Arc<Shared>;

pub fn handlers() -> impl Fn(Invoke<Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        get_state,
        get_card,
        get_screenshot,
        capture,
        sign_in,
        sign_out,
        clear_history,
        edit_sources,
        reload_sources,
        dismiss_notice,
        open_entry,
        open_main,
        copy_text,
        card_resize,
        card_close,
        open_settings,
        open_usage,
        save_settings,
        set_api_key,
        list_models,
        source_sign_in,
        source_sign_out,
        add_source,
        remove_source,
        search_catalog,
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
        .map(|jpeg| crate::model::data_url(&jpeg)))
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
}

/// Opens Settings in the main window, optionally searching sources for `search`.
#[tauri::command]
fn open_settings(shared: State<'_, SharedState>, search: Option<String>) {
    shared.send(Command::OpenSettings(search));
}

/// Opens the usage page for the problem the card shows. The page cannot
/// choose the address; it comes from the problem the app created.
#[tauri::command]
fn open_usage(shared: State<'_, SharedState>) -> Result<(), String> {
    let url = card::usage_url(&shared).ok_or("There is no usage page to open.")?;
    open::that_detached(url).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_settings(shared: State<'_, SharedState>, settings: Settings) {
    shared.send(Command::SaveSettings(settings));
}

/// Saves an API key, or removes it when `key` is empty. The key goes straight
/// to encrypted storage and is never sent back to a page.
#[tauri::command]
fn set_api_key(shared: State<'_, SharedState>, provider: Provider, key: Option<String>) {
    shared.send(Command::SetApiKey(provider, key));
}

#[tauri::command]
fn list_models(shared: State<'_, SharedState>, provider: Provider) {
    shared.send(Command::ListModels(provider));
}

#[tauri::command]
fn source_sign_in(shared: State<'_, SharedState>, name: String) {
    shared.send(Command::SourceSignIn(name));
}

#[tauri::command]
fn source_sign_out(shared: State<'_, SharedState>, name: String) {
    shared.send(Command::SourceSignOut(name));
}

/// Adds a catalog entry. Secret values go to encrypted storage, not mcp.json.
#[tauri::command]
fn add_source(
    shared: State<'_, SharedState>,
    entry: crate::sources::catalog::Entry,
    values: serde_json::Map<String, serde_json::Value>,
) {
    shared.send(Command::AddSource(Box::new(entry), values));
}

#[tauri::command]
fn remove_source(shared: State<'_, SharedState>, name: String) {
    shared.send(Command::RemoveSource(name));
}

#[tauri::command]
fn search_catalog(shared: State<'_, SharedState>, query: String) {
    shared.send(Command::SearchCatalog(query));
}
