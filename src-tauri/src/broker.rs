//! Локальный брокер сессий: unix-сокет, через который kai работает с базами
//! силами GUI-процесса (vault уже разблокирован, туннели уже подняты).
//!
//! Модуль намеренно не знает про Tauri — хост отдаёт ему [`BrokerHooks`]
//! (снапшот GUI-сессий + колбэк «состав cli-сессий изменился»), поэтому тот же
//! сервер можно поселить в отдельный демон, не меняя протокол и клиентов.
//!
//! Протокол: JSON-line на запрос, JSON-line на ответ.
//!   → {"id":1,"method":"query","params":{"profileId":"…","sql":"…"}}
//!   ← {"id":1,"result":{…}} | {"id":1,"error":"…","code":"vault_locked"}

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
#[cfg(unix)]
use serde_json::{json, Value};
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use tokio_postgres::NoTls;

use crate::db::{self, TxStatus};
use crate::error::AppError;
use crate::fsio;
#[cfg(unix)]
use crate::logging;
#[cfg(unix)]
use crate::store;
#[cfg(unix)]
use crate::vault;

pub const PROTOCOL_VERSION: u32 = 1;

/// Как долго cli-сессия живёт без запросов, прежде чем брокер её закроет.
const CLI_IDLE_TTL_SEC: u64 = 15 * 60;
/// TTL ssh-мастера для туннелей cli-сессий (общий с kai механизм mux).
const CLI_MUX_TTL: u32 = 300;

pub fn socket_path() -> Result<PathBuf, AppError> {
    fsio::config_path("broker.sock")
}

/// Removes a stale socket file and binds a fresh 0600 listener.
#[cfg(unix)]
pub fn bind() -> Result<UnixListener, AppError> {
    let path = socket_path()?;
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(listener)
}

// --- Protocol -------------------------------------------------------------------

#[derive(Deserialize)]
pub struct Request {
    #[serde(default)]
    pub id: u64,
    #[serde(flatten)]
    pub method: Method,
}

#[derive(Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Method {
    /// Рукопожатие: сверка протокола, версия сервера, состояние vault.
    Hello {
        #[serde(default)]
        client_version: String,
    },
    /// Все живые сессии: GUI-шные и cli-шные.
    Sessions,
    /// Выполнить SQL на cli-сессии профиля (открывается лениво, переживает
    /// выход kai). `write` временно снимает session-wide read-only.
    Query {
        #[serde(rename = "profileId")]
        profile_id: String,
        sql: String,
        #[serde(default = "default_max_rows", rename = "maxRows")]
        max_rows: usize,
        #[serde(default)]
        write: bool,
        /// Вернуть и типы колонок (Parse) — для типизированного --json в kai.
        #[serde(default, rename = "withTypes")]
        with_types: bool,
    },
    /// Отменить запрос, бегущий на cli-сессии профиля.
    Cancel {
        #[serde(rename = "profileId")]
        profile_id: String,
    },
}

fn default_max_rows() -> usize {
    1000
}

/// Одна строка ответа `sessions` (и полезная нагрузка `list_cli_sessions`).
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BrokerSessionInfo {
    pub profile_id: String,
    pub profile_name: String,
    /// "gui" — сессия открыта пользователем в приложении, "cli" — брокером
    /// по запросу kai.
    pub origin: String,
    pub server_version: String,
    pub tunnel_port: Option<u16>,
    pub tx: String,
    /// Сколько секунд cli-сессия простаивает (None у GUI-сессий).
    pub idle_sec: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloReply {
    pub protocol: u32,
    pub server_version: String,
    pub vault_unlocked: bool,
}

/// Колонки одного стейтмента как (имя, oid типа) — kai восстанавливает
/// `Type::from_oid` на своей стороне.
pub type WireColumnTypes = Vec<Option<Vec<(String, u32)>>>;

// --- State ----------------------------------------------------------------------

pub struct CliEntry {
    pub session: db::Session,
    /// Сериализует запросы к одной сессии (двум kai одновременно нельзя).
    busy: tokio::sync::Mutex<()>,
    last_used: Mutex<Instant>,
}

/// Владелец cli-сессий. std-Mutex, чтобы состав можно было чистить из
/// синхронных мест хоста (vault_lock); сами запросы держат только `busy`.
#[derive(Default)]
pub struct BrokerState {
    cli: Mutex<HashMap<String, Arc<CliEntry>>>,
}

impl BrokerState {
    /// Снимок cli-сессий (для `sessions` и для фронтенда GUI).
    pub fn cli_sessions(&self) -> Vec<BrokerSessionInfo> {
        self.cli
            .lock()
            .unwrap()
            .values()
            .map(|e| BrokerSessionInfo {
                profile_id: e.session.profile_id.clone(),
                profile_name: e.session.profile_name.clone(),
                origin: "cli".into(),
                server_version: e.session.server_version.clone(),
                tunnel_port: e.session.tunnel_port,
                tx: TxStatus::from_u8(e.session.tx.load(Ordering::Relaxed))
                    .as_str()
                    .into(),
                idle_sec: Some(e.last_used.lock().unwrap().elapsed().as_secs()),
            })
            .collect()
    }

    /// Закрывает все cli-сессии (lock vault, выход). true = что-то закрыли.
    pub fn clear(&self) -> bool {
        let mut map = self.cli.lock().unwrap();
        let had = !map.is_empty();
        map.clear();
        had
    }

    /// Убирает мёртвые и простоявшие дольше TTL сессии. true = что-то убрали.
    fn sweep(&self) -> bool {
        let mut map = self.cli.lock().unwrap();
        let before = map.len();
        map.retain(|_, e| {
            !e.session.client.is_closed()
                && e.last_used.lock().unwrap().elapsed().as_secs() < CLI_IDLE_TTL_SEC
        });
        map.len() != before
    }

    fn get_live(&self, profile_id: &str) -> Option<Arc<CliEntry>> {
        let map = self.cli.lock().unwrap();
        map.get(profile_id)
            .filter(|e| !e.session.client.is_closed())
            .cloned()
    }

    fn drop_entry(&self, profile_id: &str) {
        self.cli.lock().unwrap().remove(profile_id);
    }
}

/// Хост-специфика: как посмотреть GUI-сессии и как сообщить интерфейсу об
/// изменениях. В Tauri это AppState-снапшот и emit события; в будущем демоне —
/// пустой список и no-op.
pub struct BrokerHooks {
    pub gui_sessions: Box<dyn Fn() -> Vec<BrokerSessionInfo> + Send + Sync>,
    pub changed: Box<dyn Fn() + Send + Sync>,
}

// --- Server ---------------------------------------------------------------------

#[cfg(unix)]
pub async fn serve(listener: UnixListener, state: Arc<BrokerState>, hooks: Arc<BrokerHooks>) {
    {
        // фоновая чистка простаивающих cli-сессий
        let state = state.clone();
        let hooks = hooks.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                if state.sweep() {
                    (hooks.changed)();
                }
            }
        });
    }
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = state.clone();
                let hooks = hooks.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, state, hooks).await {
                        logging::log("broker", &format!("client connection error: {e}"));
                    }
                });
            }
            Err(e) => {
                logging::log("broker", &format!("accept failed: {e}"));
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

#[cfg(unix)]
async fn handle_conn(
    stream: UnixStream,
    state: Arc<BrokerState>,
    hooks: Arc<BrokerHooks>,
) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let id = req.id;
                match dispatch(req.method, &state, &hooks).await {
                    Ok(result) => json!({ "id": id, "result": result }),
                    Err((code, msg)) => json!({ "id": id, "error": msg, "code": code }),
                }
            }
            Err(e) => json!({ "id": 0, "error": format!("bad request: {e}"), "code": "protocol" }),
        };
        let mut buf = serde_json::to_vec(&reply).unwrap_or_else(|_| b"{}".to_vec());
        buf.push(b'\n');
        write.write_all(&buf).await?;
    }
    Ok(())
}

#[cfg(unix)]
type MethodError = (&'static str, String);

#[cfg(unix)]
async fn dispatch(
    method: Method,
    state: &Arc<BrokerState>,
    hooks: &Arc<BrokerHooks>,
) -> Result<Value, MethodError> {
    match method {
        Method::Hello { client_version } => {
            if !client_version.is_empty() && client_version != env!("CARGO_PKG_VERSION") {
                logging::log(
                    "broker",
                    &format!(
                        "hello from kai {client_version} (gui {})",
                        env!("CARGO_PKG_VERSION")
                    ),
                );
            }
            Ok(json!(HelloReply {
                protocol: PROTOCOL_VERSION,
                server_version: env!("CARGO_PKG_VERSION").into(),
                vault_unlocked: vault::is_unlocked(),
            }))
        }
        Method::Sessions => {
            let mut list = (hooks.gui_sessions)();
            list.extend(state.cli_sessions());
            Ok(json!(list))
        }
        Method::Query {
            profile_id,
            sql,
            max_rows,
            write,
            with_types,
        } => do_query(state, hooks, &profile_id, &sql, max_rows, write, with_types).await,
        Method::Cancel { profile_id } => {
            let entry = state
                .get_live(&profile_id)
                .ok_or(("no_session", "нет cli-сессии этого профиля".to_string()))?;
            entry
                .session
                .cancel
                .clone()
                .cancel_query(NoTls)
                .await
                .map_err(|e| ("cancel", e.to_string()))?;
            Ok(json!({}))
        }
    }
}

#[cfg(unix)]
async fn do_query(
    state: &Arc<BrokerState>,
    hooks: &Arc<BrokerHooks>,
    profile_id: &str,
    sql: &str,
    max_rows: usize,
    write: bool,
    with_types: bool,
) -> Result<Value, MethodError> {
    if !vault::is_unlocked() {
        // kai по этому коду откатывается на автономный путь со своей
        // цепочкой разблокировки
        return Err(("vault_locked", "vault заблокирован в GUI".into()));
    }
    let entry = get_or_open(state, hooks, profile_id)
        .await
        .map_err(|e| ("connect", e.to_string()))?;
    let _busy = entry.busy.lock().await;
    *entry.last_used.lock().unwrap() = Instant::now();

    let client = &entry.session.client;
    // Сессия постоянно read-only; --write снимает флаг только на время запроса.
    if write {
        let _ = db::execute(client, "SET default_transaction_read_only = off", 1).await;
    }
    let before = TxStatus::from_u8(entry.session.tx.load(Ordering::Relaxed));
    let result = db::execute(client, sql, max_rows.clamp(1, 100_000)).await;
    entry.session.tx.store(
        db::advance_tx(before, sql, result.is_ok()) as u8,
        Ordering::Relaxed,
    );
    if write {
        let _ = db::execute(client, "SET default_transaction_read_only = on", 1).await;
    }
    *entry.last_used.lock().unwrap() = Instant::now();

    match result {
        Ok(exec) => {
            let column_types: Option<WireColumnTypes> = if with_types {
                Some(
                    db::statement_column_types(client, sql)
                        .await
                        .into_iter()
                        .map(|cols| {
                            cols.map(|cols| {
                                cols.into_iter()
                                    .map(|(name, ty)| (name, ty.oid()))
                                    .collect()
                            })
                        })
                        .collect(),
                )
            } else {
                None
            };
            Ok(json!({ "exec": exec, "columnTypes": column_types }))
        }
        Err(e) => {
            // сессия могла умереть под запросом — выкинуть, следующий запрос
            // откроет свежую
            if entry.session.client.is_closed() {
                state.drop_entry(profile_id);
                (hooks.changed)();
            }
            Err(("query", e.to_string()))
        }
    }
}

/// Живая cli-сессия профиля; открывает новую (read-only, через mux-туннель),
/// когда её нет или прежняя умерла.
#[cfg(unix)]
async fn get_or_open(
    state: &Arc<BrokerState>,
    hooks: &Arc<BrokerHooks>,
    profile_id: &str,
) -> Result<Arc<CliEntry>, AppError> {
    if let Some(entry) = state.get_live(profile_id) {
        return Ok(entry);
    }
    let profile = store::find_profile(profile_id)?;
    let connected = db::connect(
        &profile,
        db::ConnectOptions {
            ssh_mux_ttl: Some(CLI_MUX_TTL),
            ..Default::default()
        },
    )
    .await?;
    let _ = db::execute(
        &connected.session.client,
        "SET default_transaction_read_only = on",
        1,
    )
    .await;
    let entry = Arc::new(CliEntry {
        session: connected.session,
        busy: tokio::sync::Mutex::new(()),
        last_used: Mutex::new(Instant::now()),
    });
    let winner = {
        let mut map = state.cli.lock().unwrap();
        // гонка двух kai: если параллельный открыватель успел раньше и его
        // сессия жива — наша лишняя, отдаём его
        match map.get(profile_id) {
            Some(existing) if !existing.session.client.is_closed() => existing.clone(),
            _ => {
                map.insert(profile_id.to_string(), entry.clone());
                entry
            }
        }
    };
    logging::log(
        "broker",
        &format!("\"{}\": cli-сессия открыта по запросу kai", profile.name),
    );
    (hooks.changed)();
    Ok(winner)
}
