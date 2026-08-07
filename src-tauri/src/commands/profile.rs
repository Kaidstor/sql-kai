//! Connection-profile CRUD. Deleting a profile also tears down its live
//! sessions (and their ssh tunnels).

use tauri::State;

use crate::error::AppError;
use crate::store::{self, Profile};

use super::AppState;

#[tauri::command]
pub fn list_profiles() -> Result<Vec<Profile>, AppError> {
    let mut marks = store::load_last_connected().unwrap_or_default();
    Ok(store::load_profiles()?
        .into_iter()
        .map(|p| p.for_frontend(&mut marks))
        .collect())
}

#[tauri::command]
pub fn save_profile(
    profile: Profile,
    password: Option<String>,
    ssh_passphrase: Option<String>,
) -> Result<Profile, AppError> {
    store::upsert_profile(profile, password, ssh_passphrase)
}

#[tauri::command]
pub fn duplicate_profile(id: String) -> Result<Profile, AppError> {
    store::duplicate_profile(&id)
}

#[tauri::command]
pub fn delete_profile(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    let dropped: Vec<_> = {
        let mut sessions = state.sessions.lock().unwrap();
        let ids: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| s.profile_id == id)
            .map(|(k, _)| k.clone())
            .collect();
        ids.into_iter()
            .filter_map(|k| sessions.remove(&k))
            .collect()
    };
    drop(dropped); // teardown ssh-туннелей — вне лока
    store::delete_profile(&id)
}
