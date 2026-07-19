//! Persistence round-trips: saved queries, settings.json, query history and
//! the diagnostics log.

use crate::error::AppError;
use crate::logging;
use crate::store::{self, HistoryEntry, SavedQuery};

#[tauri::command]
pub fn list_queries() -> Result<Vec<SavedQuery>, AppError> {
    store::load_queries()
}

#[tauri::command]
pub fn save_query(query: SavedQuery) -> Result<SavedQuery, AppError> {
    store::upsert_query(query)
}

#[tauri::command]
pub fn delete_query(id: String) -> Result<(), AppError> {
    store::delete_query(&id)
}

#[tauri::command]
pub fn get_settings() -> Result<serde_json::Value, AppError> {
    store::load_settings()
}

#[tauri::command]
pub fn save_settings(settings: serde_json::Value) -> Result<(), AppError> {
    store::save_settings(&settings)
}

/// Where settings.json lives — shown in the UI so the file is easy to find
/// (it's meant to be copied between machines).
#[tauri::command]
pub fn settings_path() -> Result<String, AppError> {
    Ok(store::settings_path()?.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn list_history() -> Result<Vec<HistoryEntry>, AppError> {
    store::load_history()
}

#[tauri::command]
pub fn record_history(entry: HistoryEntry) -> Result<Vec<HistoryEntry>, AppError> {
    store::record_history(entry)
}

#[tauri::command]
pub fn delete_history_entry(id: String) -> Result<Vec<HistoryEntry>, AppError> {
    store::delete_history_entry(&id)
}

#[tauri::command]
pub fn clear_history() -> Result<(), AppError> {
    store::replace_history(&[])
}

#[tauri::command]
pub fn import_history(entries: Vec<HistoryEntry>) -> Result<Vec<HistoryEntry>, AppError> {
    store::import_history(entries)
}

/// Where the diagnostics log lives — shown in Settings so it's easy to find.
#[tauri::command]
pub fn log_path() -> Result<String, AppError> {
    Ok(logging::log_path()?.to_string_lossy().into_owned())
}

/// Frontend-observed connection events land in the same log, so backend and
/// UI views of a drop can be correlated on one timeline.
#[tauri::command]
pub fn log_event(message: String) {
    logging::log("ui", &message);
}

/// Tail of the diagnostics log for the in-app viewer (menu → Diagnostics Log).
#[tauri::command]
pub fn read_log() -> Result<String, AppError> {
    logging::read_tail(256 * 1024)
}
