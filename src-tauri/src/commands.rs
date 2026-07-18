use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use tokio_postgres::{Client, NoTls};

use crate::db::{self, cell, cell_bool, ExecResult, Session, StatementResult, TxStatus};
use crate::error::AppError;
use crate::logging;
use crate::store::{self, HistoryEntry, Profile, SavedQuery};
use crate::vault;

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
fn client_of(state: &State<'_, AppState>, session_id: &str) -> Result<Arc<Client>, AppError> {
    Ok(client_and_tx(state, session_id)?.0)
}

/// Live client + its tx-state handle under a single lock, with the same
/// dead-client teardown as [`client_of`]. `execute_sql` needs both and would
/// otherwise lock the session map twice.
fn client_and_tx(
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
pub fn list_profiles() -> Result<Vec<Profile>, AppError> {
    Ok(store::load_profiles()?
        .into_iter()
        .map(|p| p.for_frontend())
        .collect())
}

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

/// Live cli-brokered sessions — drives the "cli" badges in the frontend.
#[tauri::command]
pub fn list_cli_sessions(
    broker: State<'_, std::sync::Arc<crate::broker::BrokerState>>,
) -> Vec<crate::broker::BrokerSessionInfo> {
    broker.cli_sessions()
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
        ids.into_iter().filter_map(|k| sessions.remove(&k)).collect()
    };
    drop(dropped); // teardown ssh-туннелей — вне лока
    store::delete_profile(&id)
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
    store::save_history(&[])
}

#[tauri::command]
pub fn import_history(entries: Vec<HistoryEntry>) -> Result<Vec<HistoryEntry>, AppError> {
    store::import_history(entries)
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

/// Connects with the given (possibly unsaved) profile, checks the DB responds, tears down.
#[tauri::command]
pub async fn test_profile(
    profile: Profile,
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

#[tauri::command]
pub async fn execute_sql(
    state: State<'_, AppState>,
    session_id: String,
    sql: String,
    max_rows: Option<usize>,
    auto_begin: Option<bool>,
) -> Result<ExecResult, AppError> {
    let (client, tx) = client_and_tx(&state, &session_id)?;
    let before = TxStatus::from_u8(tx.load(Ordering::Relaxed));
    // Manual-commit mode: hold a transaction open across runs by opening one
    // when the connection is idle, so the user never has to type BEGIN.
    let prepended = auto_begin.unwrap_or(false) && before == TxStatus::Idle;
    let sql = if prepended { format!("BEGIN;\n{sql}") } else { sql };
    let mut result = db::execute(&client, &sql, max_rows.unwrap_or(1000).clamp(1, 100_000)).await;
    // Track the transaction state whether the batch succeeded or failed — a
    // failed statement inside a tx leaves it aborted, which the badge surfaces.
    tx.store(db::advance_tx(before, &sql, result.is_ok()) as u8, Ordering::Relaxed);
    // Hide the synthetic BEGIN's result: the frontend numbers result blocks by
    // the statements of the SQL it sent (per-statement export relies on it),
    // and an "OK" block for a BEGIN the user never typed is just noise.
    if prepended {
        if let Ok(r) = &mut result {
            if !r.results.is_empty() {
                r.results.remove(0);
            }
        }
    }
    result
}

/// Full-result export: re-runs `sql` and streams the rows of statement
/// `statement_index` into `path` (csv/json/md/xlsx) with no row limit — the
/// grid's fetch cap does not apply here. The rows stream into a sibling
/// `.part` file that replaces `path` only on success, so a failed export
/// neither leaves a half-written dump nor destroys a pre-existing file it
/// never wrote (XLSX only touches the disk in its final save).
#[tauri::command]
pub async fn export_sql(
    state: State<'_, AppState>,
    session_id: String,
    sql: String,
    statement_index: Option<usize>,
    format: String,
    path: String,
    auto_begin: Option<bool>,
) -> Result<db::ExportOutcome, AppError> {
    let format = db::ExportFormat::parse(&format)?;
    let (client, tx) = client_and_tx(&state, &session_id)?;
    let before = TxStatus::from_u8(tx.load(Ordering::Relaxed));
    // Mirror execute_sql's manual-commit wrapping — the re-run must not
    // autocommit a write that Run would have kept inside the open transaction.
    // The prepended BEGIN emits its own result set, shifting the numbering.
    let (sql, statement_index) = if auto_begin.unwrap_or(false) && before == TxStatus::Idle {
        (format!("BEGIN;\n{sql}"), statement_index.unwrap_or(0) + 1)
    } else {
        (sql, statement_index.unwrap_or(0))
    };
    let tmp = format!("{path}.part");
    let result = db::export_statement(&client, &sql, statement_index, format, &tmp).await;
    // The re-run goes through the same connection — keep the tx badge honest.
    // Only a database error can have aborted the transaction; local failures
    // (unwritable path, XLSX cap) never touched the server, so they count ok.
    let sql_ok = !matches!(&result, Err(db::ExportError::Sql(_)));
    tx.store(db::advance_tx(before, &sql, sql_ok) as u8, Ordering::Relaxed);
    match result {
        Ok(outcome) => {
            std::fs::rename(&tmp, &path)?;
            Ok(outcome)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.into())
        }
    }
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub schema: String,
    pub name: String,
    pub kind: String,
}

#[tauri::command]
pub async fn list_tables(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<TableInfo>, AppError> {
    let client = client_of(&state, &session_id)?;
    let exec = db::execute(&client, db::TABLES_SQL, 100_000).await?;
    let mut out = Vec::new();
    for row in exec.results.iter().flat_map(|r| r.rows.iter()) {
        let kind = match row.get(2).and_then(|v| v.as_deref()) {
            Some("v") => "view",
            Some("m") => "matview",
            Some("f") => "foreign",
            _ => "table",
        };
        out.push(TableInfo {
            schema: cell(row, 0),
            name: cell(row, 1),
            kind: kind.to_string(),
        });
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableColumns {
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
}

/// Column names for every user relation in one round-trip — feeds the SQL
/// editor's schema autocomplete, so only names are selected (no types/PKs).
const ALL_COLUMNS_SQL: &str = "\
SELECT n.nspname, c.relname, a.attname
FROM pg_attribute a
JOIN pg_class c ON c.oid = a.attrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r','p','v','m','f')
  AND a.attnum > 0 AND NOT a.attisdropped
  AND n.nspname NOT IN ('pg_catalog','information_schema')
  AND n.nspname NOT LIKE 'pg_toast%'
  AND n.nspname NOT LIKE 'pg_temp%'
ORDER BY n.nspname, c.relname, a.attnum";

#[tauri::command]
pub async fn list_all_columns(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<TableColumns>, AppError> {
    let client = client_of(&state, &session_id)?;
    let exec = db::execute(&client, ALL_COLUMNS_SQL, 500_000).await?;
    let mut out: Vec<TableColumns> = Vec::new();
    // Rows arrive ordered by schema+table, so grouping consecutively works.
    for row in exec.results.iter().flat_map(|r| r.rows.iter()) {
        let schema = cell(row, 0);
        let table = cell(row, 1);
        let column = cell(row, 2);
        match out.last_mut() {
            Some(last) if last.schema == schema && last.table == table => {
                last.columns.push(column);
            }
            _ => out.push(TableColumns {
                schema,
                table,
                columns: vec![column],
            }),
        }
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub is_pk: bool,
    pub default_expr: Option<String>,
    pub comment: Option<String>,
}

async fn introspect_rows(
    state: &State<'_, AppState>,
    session_id: &str,
    sql: &str,
) -> Result<Vec<Vec<Option<String>>>, AppError> {
    let client = client_of(state, session_id)?;
    db::query_rows(&client, sql).await
}

#[tauri::command]
pub async fn list_columns(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<Vec<ColumnInfo>, AppError> {
    let sql = db::columns_sql(&db::regclass_literal(&schema, &table));
    let rows = introspect_rows(&state, &session_id, &sql).await?;
    Ok(rows
        .into_iter()
        .map(|row| ColumnInfo {
            name: cell(&row, 0),
            data_type: cell(&row, 1),
            nullable: cell_bool(&row, 2),
            is_pk: cell_bool(&row, 3),
            default_expr: row[4].clone(),
            comment: row[5].clone(),
        })
        .collect())
}

/// See db::table_ddl — assembled from the catalogs (no SHOW CREATE TABLE in PG).
#[tauri::command]
pub async fn get_table_ddl(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<String, AppError> {
    let client = client_of(&state, &session_id)?;
    db::table_ddl(&client, &schema, &table).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub primary: bool,
    pub columns: Option<String>,
    pub definition: String,
}

#[tauri::command]
pub async fn list_indexes(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<Vec<IndexInfo>, AppError> {
    let sql = db::indexes_sql(&db::regclass_literal(&schema, &table));
    let rows = introspect_rows(&state, &session_id, &sql).await?;
    Ok(rows
        .into_iter()
        .map(|row| IndexInfo {
            name: cell(&row, 0),
            unique: cell_bool(&row, 1),
            primary: cell_bool(&row, 2),
            columns: row[3].clone(),
            definition: cell(&row, 4),
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationInfo {
    pub name: String,
    pub columns: Option<String>,
    pub ref_table: String,
    pub ref_columns: Option<String>,
    pub on_update: String,
    pub on_delete: String,
}

#[tauri::command]
pub async fn list_relations(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<Vec<RelationInfo>, AppError> {
    let sql = db::relations_sql(&db::regclass_literal(&schema, &table));
    let rows = introspect_rows(&state, &session_id, &sql).await?;
    Ok(rows
        .into_iter()
        .map(|row| RelationInfo {
            name: cell(&row, 0),
            columns: row[1].clone(),
            ref_table: cell(&row, 2),
            ref_columns: row[3].clone(),
            on_update: cell(&row, 4),
            on_delete: cell(&row, 5),
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerInfo {
    pub name: String,
    pub timing: String,
    pub events: String,
    pub definition: String,
    pub enabled: bool,
}

#[tauri::command]
pub async fn list_triggers(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<Vec<TriggerInfo>, AppError> {
    let sql = db::triggers_sql(&db::regclass_literal(&schema, &table));
    let rows = introspect_rows(&state, &session_id, &sql).await?;
    Ok(rows
        .into_iter()
        .map(|row| TriggerInfo {
            name: cell(&row, 0),
            timing: cell(&row, 1),
            events: cell(&row, 2),
            definition: cell(&row, 3),
            enabled: cell_bool(&row, 4),
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnumTypeInfo {
    pub schema: String,
    pub name: String,
    pub labels: Vec<String>,
}

/// Every enum type of the database with its labels — feeds filter-value
/// suggestions; enums rarely change, the frontend caches per profile.
#[tauri::command]
pub async fn list_enums(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<EnumTypeInfo>, AppError> {
    let rows = introspect_rows(&state, &session_id, db::ENUMS_SQL).await?;
    let mut out: Vec<EnumTypeInfo> = Vec::new();
    // Rows arrive ordered by schema+type, so grouping consecutively works.
    for row in rows {
        let schema = cell(&row, 0);
        let name = cell(&row, 1);
        let label = cell(&row, 2);
        match out.last_mut() {
            Some(last) if last.schema == schema && last.name == name => {
                last.labels.push(label);
            }
            _ => out.push(EnumTypeInfo {
                schema,
                name,
                labels: vec![label],
            }),
        }
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyInfo {
    pub name: String,
    pub command: String,
    pub permissive: bool,
    /// NULL = PUBLIC (pg_policy stores roles={0} for it).
    pub roles: Option<String>,
    pub using_expr: Option<String>,
    pub check_expr: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TablePolicies {
    pub rls_enabled: bool,
    pub rls_forced: bool,
    pub policies: Vec<PolicyInfo>,
}

#[tauri::command]
pub async fn list_policies(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<TablePolicies, AppError> {
    let regclass = db::regclass_literal(&schema, &table);
    let rls = introspect_rows(&state, &session_id, &db::rls_sql(&regclass)).await?;
    let rows = introspect_rows(&state, &session_id, &db::policies_sql(&regclass)).await?;
    Ok(TablePolicies {
        rls_enabled: rls.first().map(|r| cell_bool(r, 0)).unwrap_or(false),
        rls_forced: rls.first().map(|r| cell_bool(r, 1)).unwrap_or(false),
        policies: rows
            .into_iter()
            .map(|row| PolicyInfo {
                name: cell(&row, 0),
                command: cell(&row, 1),
                permissive: cell_bool(&row, 2),
                roles: row[3].clone(),
                using_expr: row[4].clone(),
                check_expr: row[5].clone(),
            })
            .collect(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TablePage {
    pub result: StatementResult,
    pub duration_ms: u64,
    pub approx_rows: i64,
}

/// One ORDER BY entry; the grid sends a list for multi-column sort.
#[derive(Deserialize)]
pub struct SortSpec {
    pub column: String,
    pub dir: Option<String>,
}

#[tauri::command]
pub async fn get_table_page(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
    limit: u32,
    offset: u64,
    sorts: Option<Vec<SortSpec>>,
    filter: Option<String>,
) -> Result<TablePage, AppError> {
    let client = client_of(&state, &session_id)?;
    let qualified = format!("{}.{}", db::quote_ident(&schema), db::quote_ident(&table));
    let limit = limit.clamp(1, 1000);

    // User-editable WHERE expression (FK navigation, filter bar) — raw SQL by
    // design, like the query editor itself.
    let where_clause = filter
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(|f| format!(" WHERE {f}"));

    let mut sql = format!("SELECT * FROM {qualified}");
    if let Some(w) = &where_clause {
        sql.push_str(w);
    }
    let order: Vec<String> = sorts
        .unwrap_or_default()
        .iter()
        .filter(|s| !s.column.is_empty())
        .map(|s| {
            let dir = match s.dir.as_deref() {
                Some("desc") | Some("DESC") => "DESC",
                _ => "ASC",
            };
            format!("{} {dir}", db::quote_ident(&s.column))
        })
        .collect();
    if !order.is_empty() {
        sql.push_str(&format!(" ORDER BY {}", order.join(", ")));
    }
    sql.push_str(&format!(" LIMIT {limit} OFFSET {offset}"));

    let exec = db::execute(&client, &sql, limit as usize).await?;
    let result = exec.results.into_iter().next().unwrap_or_default();

    let approx_rows = if let Some(w) = &where_clause {
        // Planner row estimate for the filtered set — cheap, unlike count(*).
        let explain = format!("EXPLAIN (FORMAT JSON) SELECT * FROM {qualified}{w}");
        match db::execute(&client, &explain, 10).await {
            Ok(r) => r
                .results
                .first()
                .and_then(|res| res.rows.first())
                .and_then(|row| row[0].as_deref())
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .and_then(|v| v.get(0)?.get("Plan")?.get("Plan Rows")?.as_i64())
                .unwrap_or(-1),
            Err(_) => -1,
        }
    } else {
        let approx_sql = format!(
            "SELECT reltuples::bigint FROM pg_class WHERE oid = {}::regclass",
            db::quote_literal(&qualified)
        );
        db::query_scalar(&client, &approx_sql)
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(-1)
    };

    Ok(TablePage {
        result,
        duration_ms: exec.duration_ms,
        approx_rows,
    })
}

/// Writes an exported result set to the path picked in the save dialog.
#[tauri::command]
pub fn save_text_file(path: String, contents: String) -> Result<(), AppError> {
    std::fs::write(&path, contents)?;
    Ok(())
}

/// Copies text marked as concealed (`org.nspasteboard.ConcealedType` on macOS)
/// so clipboard-history managers skip it.
#[tauri::command]
pub fn copy_text_concealed(text: String) -> Result<(), AppError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| AppError::Msg(e.to_string()))?;
    let set = clipboard.set();
    #[cfg(target_os = "macos")]
    let set = arboard::SetExtApple::exclude_from_history(set);
    set.text(text).map_err(|e| AppError::Msg(e.to_string()))
}

/// Ответ webview на событие `agent://gui-request` (метод gui_selection
/// брокера, MCP-tool `selection`). Опоздавший ответ (хук уже снял отправителя
/// по таймауту) молча игнорируется.
#[tauri::command]
pub fn agent_gui_reply(state: State<AppState>, id: String, payload: serde_json::Value) {
    if let Some(tx) = state.gui_requests.lock().unwrap().remove(&id) {
        let _ = tx.send(payload);
    }
}

/// Абсолютный путь к бандл-CLI (`sql-kai-cli` рядом с бинарём приложения) —
/// команда MCP-сервера, который панель агента передаёт в ACP session/new.
/// None — сборка без sidecar (dev до первого cargo build --bins).
#[tauri::command]
pub fn cli_bin_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let src = exe.parent()?.join("sql-kai-cli");
    src.exists().then(|| src.display().to_string())
}

/// Installs the `sql-kai` CLI into the system PATH, "big-company" style
/// (Zed / VS Code): symlinks the `sql-kai-cli` sidecar bundled next to the
/// running app into `/usr/local/bin` — which is always on PATH via
/// `/etc/paths`. The symlink points into the .app, so future app updates
/// carry the CLI along. Tries a direct symlink first (writable Homebrew
/// setups) and falls back to an admin prompt (password / Touch ID) when the
/// dir is root-owned. Returns the created path; the sentinel error
/// `"cancelled"` means the user dismissed the auth dialog.
#[tauri::command]
pub fn install_cli() -> Result<String, AppError> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::symlink;
        use std::path::Path;

        let exe = std::env::current_exe()?;
        let src = exe
            .parent()
            .map(|p| p.join("sql-kai-cli"))
            .ok_or_else(|| AppError::Msg("не удалось определить путь к бандлу".into()))?;
        if !src.exists() {
            return Err(AppError::Msg(format!(
                "CLI-бинарь не найден рядом с приложением: {}\n\
                 Нужна версия sql-kai со встроенным sql-kai-cli (sidecar).",
                src.display()
            )));
        }
        let target = Path::new("/usr/local/bin/sql-kai");

        // Fast path: recreate the symlink directly when /usr/local/bin is
        // writable (e.g. Homebrew) — no password prompt needed.
        let _ = std::fs::remove_file(target); // ignore "not found" / "denied"
        if symlink(&src, target).is_ok() {
            return Ok(target.display().to_string());
        }

        // Slow path: the dir is root-owned. Escalate via the native auth
        // dialog; `ln -sf` handles a pre-existing root-owned symlink.
        let script = format!(
            "do shell script \"mkdir -p /usr/local/bin && ln -sf '{}' '{}'\" \
             with administrator privileges",
            src.display(),
            target.display()
        );
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()?;
        if out.status.success() {
            return Ok(target.display().to_string());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        // -128 == user dismissed the auth dialog.
        if stderr.contains("-128") || stderr.contains("User canceled") {
            return Err(AppError::Msg("cancelled".into()));
        }
        Err(AppError::Msg(format!(
            "не удалось создать симлинк: {}",
            stderr.trim()
        )))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(AppError::Msg(
            "Install CLI поддерживается только на macOS".into(),
        ))
    }
}

/// Relaunch to apply an update. plugin-process relaunch() spawns the binary
/// directly, bypassing LaunchServices — modern macOS denies activation to such
/// a process, so the new window starts behind others. `open -n` relaunches the
/// bundle as a user-initiated launch and the window comes to front.
#[tauri::command]
pub fn relaunch_app(app: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        // …/sql-kai.app/Contents/MacOS/sql-kai → …/sql-kai.app
        let bundle = std::env::current_exe().ok().and_then(|exe| {
            let b = exe.ancestors().nth(3)?;
            b.extension()
                .is_some_and(|e| e == "app")
                .then(|| b.to_path_buf())
        });
        if let Some(bundle) = bundle {
            let spawned = std::process::Command::new("open")
                .arg("-n")
                .arg(&bundle)
                .spawn()
                .is_ok();
            if spawned {
                app.exit(0);
                return;
            }
        }
    }
    // Outside a bundle (dev run) or non-macOS — plain restart.
    app.restart();
}

