use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;
use tokio_postgres::{Client, NoTls};

use crate::db::{self, cell, cell_bool, ExecResult, Session, StatementResult};
use crate::error::AppError;
use crate::logging;
use crate::store::{self, HistoryEntry, Profile, SavedQuery};
use crate::vault;

#[derive(Default)]
pub struct AppState {
    pub sessions: Mutex<HashMap<String, Session>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub profile_id: String,
    pub server_version: String,
    pub tunnel_port: Option<u16>,
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
        .ok_or_else(|| AppError::Msg("session not found (already disconnected?)".into()))?;
    Ok(f(session))
}

/// Client of a live session. A closed client can't recover, so it is removed
/// here (tearing the tunnel down with it) and every command reports the same
/// "connection lost" error the frontend recognises to offer a reconnect.
fn client_of(state: &State<'_, AppState>, session_id: &str) -> Result<Arc<Client>, AppError> {
    let mut sessions = state.sessions.lock().unwrap();
    let session = sessions
        .get(session_id)
        .ok_or_else(|| AppError::Msg("session not found (already disconnected?)".into()))?;
    let client = session.client.clone();
    if client.is_closed() {
        let name = session.profile_name.clone();
        sessions.remove(session_id);
        logging::log(
            "session",
            &format!("\"{name}\": dead client detected on API call — session dropped"),
        );
        return Err(AppError::Msg(
            "connection lost (tunnel or server dropped) — reconnect the profile".into(),
        ));
    }
    Ok(client)
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
    pub biometrics_supported: bool,
    /// A DEK copy is enrolled in the biometric keychain for this vault.
    pub biometrics_enrolled: bool,
}

#[tauri::command]
pub fn vault_status() -> VaultStatus {
    VaultStatus {
        exists: vault::exists(),
        unlocked: vault::is_unlocked(),
        biometrics_supported: vault::biometric_supported(),
        biometrics_enrolled: vault::biometric_enrolled(),
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
#[tauri::command]
pub fn vault_lock(state: State<'_, AppState>) {
    state.sessions.lock().unwrap().clear();
    vault::lock();
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
    state
        .sessions
        .lock()
        .unwrap()
        .retain(|_, s| s.profile_id != id);
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
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<SessionInfo, AppError> {
    let profile = store::find_profile(&profile_id)?;
    let connected = db::connect(&profile, db::ConnectOptions::default()).await?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let info = SessionInfo {
        session_id: session_id.clone(),
        profile_id,
        server_version: connected.server_version,
        tunnel_port: connected.tunnel_port,
    };
    state
        .sessions
        .lock()
        .unwrap()
        .insert(session_id, connected.session);
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
) -> Result<ExecResult, AppError> {
    let client = client_of(&state, &session_id)?;
    db::execute(&client, &sql, max_rows.unwrap_or(1000).clamp(1, 100_000)).await
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
pub async fn get_tables(
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
pub async fn get_all_columns(
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
pub async fn get_columns(
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
pub async fn get_indexes(
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
pub async fn get_relations(
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
}

#[tauri::command]
pub async fn get_triggers(
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
        })
        .collect())
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
        match db::execute(&client, &approx_sql, 1).await {
            Ok(r) => r
                .results
                .first()
                .and_then(|res| res.rows.first())
                .and_then(|row| row[0].as_deref())
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(-1),
            Err(_) => -1,
        }
    };

    Ok(TablePage {
        result,
        duration_ms: exec.duration_ms,
        approx_rows,
    })
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

