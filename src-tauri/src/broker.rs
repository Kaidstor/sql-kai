//! Локальный брокер сессий: unix-сокет, через который kai работает с базами
//! силами GUI-процесса (vault уже разблокирован, туннели уже подняты).
//!
//! Модуль намеренно не знает про Tauri — хост отдаёт ему [`BrokerHooks`]
//! (снапшот GUI-сессий + колбэк «состав cli-сессий изменился»), поэтому тот же
//! сервер можно поселить в отдельный демон, не меняя протокол и клиентов.
//!
//! Протокол: JSON-line на запрос, JSON-line на ответ.
//!   → {"id":1,"method":"query","params":{"profileId":"…","sql":"…"}}
//!   ← {"id":1,"result":{…}} | {"id":1,"error":"…","code":"vault_locked","sqlstate":null}

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
        // Не сумели ужать права — сокет не поднимаем: через него выполняется
        // SQL под разлоченным vault.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
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

impl BrokerSessionInfo {
    /// Проекция живой сессии в строку `sessions`. `origin` — "cli"/"gui",
    /// `idle_sec` — только для cli. Единственное место маппинга Session→info
    /// (раньше дублировалось в broker::cli_sessions и в GUI-хуке lib.rs).
    pub fn from_session(s: &db::Session, origin: &str, idle_sec: Option<u64>) -> Self {
        BrokerSessionInfo {
            profile_id: s.profile_id.clone(),
            profile_name: s.profile_name.clone(),
            origin: origin.into(),
            server_version: s.server_version.clone(),
            tunnel_port: s.tunnel_port,
            tx: TxStatus::label_from_u8(s.tx.load(Ordering::Relaxed)).into(),
            idle_sec,
        }
    }
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
            .map(|e| {
                let idle = e.last_used.lock().unwrap().elapsed().as_secs();
                BrokerSessionInfo::from_session(&e.session, "cli", Some(idle))
            })
            .collect()
    }

    /// Закрывает все cli-сессии (lock vault, выход). true = что-то закрыли.
    /// Teardown сессий (kill ssh-туннеля) — вне лока, чтобы не держать
    /// остальных на syscall'ах.
    pub fn clear(&self) -> bool {
        let drained: Vec<Arc<CliEntry>> = {
            let mut map = self.cli.lock().unwrap();
            map.drain().map(|(_, e)| e).collect()
        };
        let had = !drained.is_empty();
        drop(drained);
        had
    }

    /// Убирает мёртвые и простоявшие дольше TTL сессии. true = что-то убрали.
    fn sweep(&self) -> bool {
        let mut dead: Vec<Arc<CliEntry>> = Vec::new();
        let removed = {
            let mut map = self.cli.lock().unwrap();
            let before = map.len();
            map.retain(|_, e| {
                let live = !e.session.client.is_closed()
                    && e.last_used.lock().unwrap().elapsed().as_secs() < CLI_IDLE_TTL_SEC;
                if !live {
                    dead.push(e.clone());
                }
                live
            });
            map.len() != before
        };
        drop(dead); // последние Arc-рефы гаснут вне лока
        removed
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
                    Err(e) => json!({
                        "id": id, "error": e.message, "code": e.code, "sqlstate": e.sqlstate,
                    }),
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
struct MethodError {
    code: &'static str,
    message: String,
    /// SQLSTATE серверной ошибки (например 25006 read-only) — kai по нему
    /// показывает hint, не разбирая текст. Старые клиенты поле игнорируют.
    sqlstate: Option<String>,
}

#[cfg(unix)]
fn method_err(code: &'static str, message: impl Into<String>) -> MethodError {
    MethodError { code, message: message.into(), sqlstate: None }
}

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
                .ok_or_else(|| method_err("no_session", "нет cli-сессии этого профиля"))?;
            entry
                .session
                .cancel
                .clone()
                .cancel_query(NoTls)
                .await
                .map_err(|e| method_err("cancel", e.to_string()))?;
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
        return Err(method_err("vault_locked", "vault заблокирован в GUI"));
    }
    let entry = get_or_open(state, hooks, profile_id)
        .await
        .map_err(|e| method_err("connect", e.to_string()))?;
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
            Err(MethodError {
                code: "query",
                message: e.to_string(),
                sqlstate: e.sqlstate().map(str::to_string),
            })
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
            ssh_mux_ttl: Some(crate::tunnel::DEFAULT_MUX_TTL),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_query_request_line() {
        let line = r#"{"id":7,"method":"query","params":{"profileId":"p1","sql":"SELECT 1","write":true}}"#;
        let req: Request = serde_json::from_str(line).unwrap();
        assert_eq!(req.id, 7);
        match req.method {
            Method::Query {
                profile_id,
                sql,
                max_rows,
                write,
                with_types,
            } => {
                assert_eq!(profile_id, "p1");
                assert_eq!(sql, "SELECT 1");
                assert_eq!(max_rows, 1000); // default
                assert!(write);
                assert!(!with_types);
            }
            _ => panic!("wrong method"),
        }
    }

    /// Живой round-trip через настоящий unix-сокет: hello и sessions (пустое
    /// состояние, без БД). Также фиксирует, что params: null и отсутствующий
    /// params валидны для unit-вариантов.
    #[cfg(unix)]
    #[tokio::test]
    async fn hello_and_sessions_over_socket() {
        let path = std::env::temp_dir()
            .join(format!("kai-broker-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let state = Arc::new(BrokerState::default());
        let hooks = Arc::new(BrokerHooks {
            gui_sessions: Box::new(Vec::new),
            changed: Box::new(|| {}),
        });
        tokio::spawn(serve(listener, state, hooks));

        let stream = UnixStream::connect(&path).await.unwrap();
        let (r, mut w) = stream.into_split();
        let mut lines = BufReader::new(r).lines();

        w.write_all(b"{\"id\":1,\"method\":\"hello\",\"params\":{}}\n")
            .await
            .unwrap();
        let v: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["protocol"], PROTOCOL_VERSION);

        for req in [
            "{\"id\":2,\"method\":\"sessions\"}\n".as_bytes(),
            b"{\"id\":3,\"method\":\"sessions\",\"params\":null}\n",
        ] {
            w.write_all(req).await.unwrap();
            let v: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert!(
                v["result"].as_array().is_some_and(|a| a.is_empty()),
                "unexpected reply: {v}"
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hello_roundtrip() {
        let hello = HelloReply {
            protocol: PROTOCOL_VERSION,
            server_version: "0.1.0".into(),
            vault_unlocked: true,
        };
        let json = serde_json::to_string(&hello).unwrap();
        let back: HelloReply = serde_json::from_str(&json).unwrap();
        assert_eq!(back.protocol, PROTOCOL_VERSION);
        assert!(back.vault_unlocked);
    }
}
