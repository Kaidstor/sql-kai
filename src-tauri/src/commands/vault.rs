//! Vault lifecycle: setup/unlock/lock and the Touch ID enrollment toggles.

use serde::Serialize;
use tauri::State;

use crate::error::AppError;
use crate::vault;

use super::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    /// A vault file exists on disk (i.e. a master password was set up before).
    pub exists: bool,
    /// The DEK is decrypted and held in memory for this session.
    pub unlocked: bool,
    /// This platform can offer Touch ID at all (macOS).
    pub biometric_supported: bool,
    /// A DEK copy is enrolled in the biometric keychain for this vault.
    pub biometric_enrolled: bool,
}

#[tauri::command]
pub fn vault_status() -> VaultStatus {
    VaultStatus {
        exists: vault::exists(),
        unlocked: vault::is_unlocked(),
        biometric_supported: vault::biometric_supported(),
        biometric_enrolled: vault::biometric_enrolled(),
    }
}

/// First run: create the vault protected by `password` and unlock it.
#[tauri::command]
pub fn vault_setup(password: String) -> Result<(), AppError> {
    vault::setup(&password)
}

/// Unlock an existing vault with the master password.
#[tauri::command]
pub fn vault_unlock(password: String) -> Result<(), AppError> {
    vault::unlock_password(&password)
}

/// Unlock via Touch ID. The keychain read blocks on the system prompt, so it
/// runs on a blocking thread instead of stalling the main/async runtime.
#[tauri::command]
pub async fn vault_unlock_biometric() -> Result<(), AppError> {
    tokio::task::spawn_blocking(vault::unlock_biometric)
        .await
        .map_err(|e| AppError::Msg(format!("unlock task failed: {e}")))?
}

/// Enroll the current session DEK behind Touch ID (vault must be unlocked).
#[tauri::command]
pub fn vault_enable_biometric() -> Result<(), AppError> {
    vault::enable_biometric()
}

#[tauri::command]
pub fn vault_disable_biometric() -> Result<(), AppError> {
    vault::disable_biometric()
}

/// Drop the in-memory DEK and all live sessions (secrets become unreadable).
/// Broker cli-sessions close too — lock means "nothing stays connected".
#[tauri::command]
pub fn vault_lock(
    state: State<'_, AppState>,
    broker: State<'_, std::sync::Arc<crate::broker::BrokerState>>,
) {
    let drained: Vec<_> = {
        let mut sessions = state.sessions.lock().unwrap();
        sessions.drain().map(|(_, s)| s).collect()
    };
    drop(drained); // teardown ssh-туннелей — вне лока
    broker.clear();
    vault::lock();
    // Holder (фоновый держатель cli-сессий sql-kai) тоже держит DEK в памяти —
    // lock гасит и его. Best-effort: не запущен — тишина.
    #[cfg(unix)]
    tauri::async_runtime::spawn(crate::broker::shutdown_holder());
}
