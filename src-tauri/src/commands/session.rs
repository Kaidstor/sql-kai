//! Session state of the GUI process and the commands that manage it:
//! connect/disconnect, adoption after a webview reload, tx status, cancel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{Emitter, Manager, State};
use tokio_postgres::{Client, NoTls};

use crate::db::{self, Session, TxStatus};
use crate::error::AppError;
use crate::logging;
use crate::store;

#[derive(Default)]
pub struct AppState {
    pub sessions: Mutex<HashMap<String, Session>>,
    /// Открытые запросы «спроси GUI» (событие agent://gui-request →
    /// команда agent_gui_reply): id запроса → отправитель ответа webview.
    pub gui_requests: Mutex<HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub profile_id: String,
    pub server_version: String,
    pub tunnel_port: Option<u16>,
    /// Heuristic transaction state: "idle" | "active" | "failed".
    pub tx: String,
    /// True for a per-tab secondary connection (own pid / transaction).
    pub isolated: bool,
    /// Backend pid — filled for isolated sessions so the tab can show it.
    pub pid: Option<i32>,
}

/// Payload of `session://lost` — pushed the instant a session's wire dies.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionLostEvent {
    session_id: String,
    profile_id: String,
    reason: String,
}

/// Inserts a fresh session into the map and arms its "wire died" push (see
/// Session::closed_rx): the moment the pg connection future resolves — ssh
/// tunnel killed, network drop, server gone — the dead session is dropped
/// (tearing its tunnel down) and `session://lost` reaches the frontend, so
/// the loss is visible at once instead of on the next query. A deliberate
/// disconnect aborts the sender, so no event fires then. Insert-then-watch,
/// in that order — a wire already dead at spawn time is found and removed.
fn insert_and_watch(
    app: &tauri::AppHandle,
    state: &State<'_, AppState>,
    session_id: &str,
    mut session: Session,
) {
    let rx = session.closed_rx.take();
    let session_id = session_id.to_string();
    let profile_id = session.profile_id.clone();
    let name = session.profile_name.clone();
    state
        .sessions
        .lock()
        .unwrap()
        .insert(session_id.clone(), session);
    let Some(rx) = rx else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(reason) = rx.await else {
            return; // sender aborted — the app closed the session itself
        };
        let removed = {
            let state = app.state::<AppState>();
            let mut sessions = state.sessions.lock().unwrap();
            sessions.remove(&session_id)
        };
        if removed.is_some() {
            logging::log(
                "session",
                &format!("\"{name}\": session dropped right after connection loss"),
            );
        }
        drop(removed); // teardown туннеля — вне лока
        let _ = app.emit(
            "session://lost",
            SessionLostEvent {
                session_id,
                profile_id,
                reason,
            },
        );
    });
}

/// Интервал keepalive-пингов GUI-сессий. Простаивающий pg-провод режут
/// внешние idle-таймауты (conntrack/файрвол на участке bastion→DB,
/// балансировщики, idle_session_timeout сервера) — ssh ServerAliveInterval
/// покрывает только участок клиент↔бастион. 30с — ниже типичных минимальных
/// таймаутов (60с у балансировщиков).
const KEEPALIVE_EVERY: Duration = Duration::from_secs(30);

/// Просроченный пинг просто бросается: сессия, занятая длинным запросом,
/// не мёртвая (её провод и так активен), а мёртвую обнаружит closed_rx-вотчер.
const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Держит GUI-сессии (включая изолированные) живыми на простое: раз в
/// [`KEEPALIVE_EVERY`] пингует каждую пустым запросом — один round-trip по
/// всему пути до Postgres, не влияющий на состояние транзакции (даже
/// aborted). CLI-сессий брокера не касается — у тех намеренный idle-TTL.
/// Ошибки пинга не обрабатываются: умершую сессию убирает и анонсирует
/// (`session://lost`) вотчер из [`insert_and_watch`].
pub fn spawn_keepalive(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(KEEPALIVE_EVERY).await;
            let clients: Vec<Arc<Client>> = {
                let state = app.state::<AppState>();
                let sessions = state.sessions.lock().unwrap();
                sessions.values().map(|s| s.client.clone()).collect()
            };
            // Каждый пинг — своей таской: зависший провод одной сессии не
            // должен задерживать пинги остальных.
            for client in clients.into_iter().filter(|c| !c.is_closed()) {
                tauri::async_runtime::spawn(async move {
                    let _ =
                        tokio::time::timeout(KEEPALIVE_TIMEOUT, client.simple_query("")).await;
                });
            }
        }
    });
}

/// Runs `f` on the live session, or errors if it was already disconnected.
fn with_session<T>(
    state: &State<'_, AppState>,
    session_id: &str,
    f: impl FnOnce(&Session) -> T,
) -> Result<T, AppError> {
    let sessions = state.sessions.lock().unwrap();
    let session = sessions
        .get(session_id)
        .ok_or(AppError::SessionGone)?;
    Ok(f(session))
}

/// Client of a live session. A closed client can't recover, so it is removed
/// here (tearing the tunnel down with it) and every command reports the same
/// "connection lost" error the frontend recognises to offer a reconnect.
pub(super) fn client_of(
    state: &State<'_, AppState>,
    session_id: &str,
) -> Result<Arc<Client>, AppError> {
    Ok(client_and_tx(state, session_id)?.0)
}

/// Live client + its tx-state handle under a single lock, with the same
/// dead-client teardown as [`client_of`]. `execute_sql` needs both and would
/// otherwise lock the session map twice.
pub(super) fn client_and_tx(
    state: &State<'_, AppState>,
    session_id: &str,
) -> Result<(Arc<Client>, Arc<AtomicU8>), AppError> {
    let mut sessions = state.sessions.lock().unwrap();
    let session = sessions
        .get(session_id)
        .ok_or(AppError::SessionGone)?;
    let client = session.client.clone();
    if client.is_closed() {
        let name = session.profile_name.clone();
        let dead = sessions.remove(session_id);
        drop(sessions); // teardown туннеля — вне лока
        drop(dead);
        logging::log(
            "session",
            &format!("\"{name}\": dead client detected on API call — session dropped"),
        );
        return Err(AppError::ConnectionLost);
    }
    Ok((client, session.tx.clone()))
}

#[tauri::command]
pub async fn connect_profile(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<SessionInfo, AppError> {
    let profile = store::profile_by_id(&profile_id)?;
    // Same ControlMaster sockets as sql-kai: whoever connects first pays for the
    // ssh auth, later tunnels from either side attach to the live master.
    let connected = db::connect(
        &profile,
        db::ConnectOptions {
            ssh_mux_ttl: Some(crate::tunnel::DEFAULT_MUX_TTL),
            ..Default::default()
        },
    )
    .await?;
    let _ = store::record_last_connected(&profile_id, store::ConnectVia::Gui);
    let session_id = uuid::Uuid::new_v4().to_string();
    let info = SessionInfo {
        session_id: session_id.clone(),
        profile_id,
        server_version: connected.server_version,
        tunnel_port: connected.tunnel_port,
        tx: TxStatus::Idle.as_str().into(),
        isolated: false,
        pid: None,
    };
    insert_and_watch(&app, &state, &session_id, connected.session);
    Ok(info)
}

/// Opens a second connection for the profile, reusing its primary session's ssh
/// tunnel (no new tunnel) — a per-tab isolated session with its own backend pid
/// and transaction. The profile must already be connected.
#[tauri::command]
pub async fn open_isolated_session(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<SessionInfo, AppError> {
    let profile = store::profile_by_id(&profile_id)?;
    // Reuse the primary session's endpoint: 127.0.0.1:<tunnel_port> when
    // tunneled, else the profile's own host:port (direct connection).
    let endpoint = {
        let sessions = state.sessions.lock().unwrap();
        let primary = sessions
            .values()
            .find(|s| s.profile_id == profile_id && !s.isolated)
            .ok_or_else(|| AppError::Msg("connect the profile first".into()))?;
        match primary.tunnel_port {
            Some(port) => ("127.0.0.1".to_string(), port),
            None => (profile.host.clone(), profile.port),
        }
    };
    let connected = db::connect(
        &profile,
        db::ConnectOptions {
            endpoint_override: Some(endpoint),
            ..Default::default()
        },
    )
    .await?;
    // Best-effort backend pid for display; not fatal if it fails.
    let pid = db::query_scalar(&connected.session.client, "SELECT pg_backend_pid()")
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i32>().ok());
    let session_id = uuid::Uuid::new_v4().to_string();
    logging::log(
        "session",
        &format!(
            "\"{}\": opened isolated session{}",
            profile.name,
            pid.map(|p| format!(" (pid {p})")).unwrap_or_default()
        ),
    );
    let info = SessionInfo {
        session_id: session_id.clone(),
        profile_id,
        server_version: connected.server_version,
        tunnel_port: connected.tunnel_port,
        tx: TxStatus::Idle.as_str().into(),
        isolated: true,
        pid,
    };
    insert_and_watch(&app, &state, &session_id, connected.session);
    Ok(info)
}

#[tauri::command]
pub fn disconnect_session(state: State<'_, AppState>, session_id: String) -> Result<(), AppError> {
    let removed = state.sessions.lock().unwrap().remove(&session_id);
    if let Some(s) = &removed {
        logging::log("session", &format!("\"{}\": disconnected", s.profile_name));
    }
    Ok(())
}

/// Live sessions, so a reloaded webview can re-adopt them instead of
/// starting from scratch (connections and tunnels survive page reloads).
#[tauri::command]
pub fn list_sessions(state: State<'_, AppState>) -> Vec<SessionInfo> {
    state
        .sessions
        .lock()
        .unwrap()
        .iter()
        .map(|(id, s)| SessionInfo {
            session_id: id.clone(),
            profile_id: s.profile_id.clone(),
            server_version: s.server_version.clone(),
            tunnel_port: s.tunnel_port,
            tx: TxStatus::label_from_u8(s.tx.load(Ordering::Relaxed)).into(),
            isolated: s.isolated,
            pid: None,
        })
        .collect()
}

/// Live cli-brokered sessions — drives the "cli" badges in the frontend.
#[tauri::command]
pub fn list_cli_sessions(
    broker: State<'_, std::sync::Arc<crate::broker::BrokerState>>,
) -> Vec<crate::broker::BrokerSessionInfo> {
    broker.cli_sessions()
}

/// Connects with the given (possibly unsaved) profile, checks the DB responds, tears down.
#[tauri::command]
pub async fn test_profile(
    profile: store::Profile,
    password: Option<String>,
    ssh_passphrase: Option<String>,
) -> Result<String, AppError> {
    let connected = db::connect(
        &profile,
        db::ConnectOptions {
            password_override: password.filter(|p| !p.is_empty()),
            ssh_passphrase_override: ssh_passphrase.filter(|p| !p.is_empty()),
            ..Default::default()
        },
    )
    .await?;
    let version = connected.server_version;
    drop(connected.session);
    Ok(if version.is_empty() {
        "connected".to_string()
    } else {
        format!("PostgreSQL {version}")
    })
}

/// Current heuristic transaction state of the session ("idle"/"active"/"failed").
/// Read after a run to refresh the status-bar badge (covers the error path,
/// where `execute_sql` returns Err and carries no result).
#[tauri::command]
pub fn session_tx_status(state: State<'_, AppState>, session_id: String) -> Result<String, AppError> {
    with_session(&state, &session_id, |s| {
        TxStatus::label_from_u8(s.tx.load(Ordering::Relaxed)).into()
    })
}

#[tauri::command]
pub async fn cancel_query(state: State<'_, AppState>, session_id: String) -> Result<(), AppError> {
    let cancel = with_session(&state, &session_id, |s| s.cancel.clone())?;
    cancel.cancel_query(NoTls).await?;
    Ok(())
}
