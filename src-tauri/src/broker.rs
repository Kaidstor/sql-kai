//! Локальный брокер сессий: unix-сокет, через который sql-kai работает с базами
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

/// …но сессия с открытой транзакцией — коротко: висящий BEGIN держит
/// блокировки и мешает vacuum'у на проде. Drop соединения её откатит.
const CLI_TX_IDLE_TTL_SEC: u64 = 180;

pub fn socket_path() -> Result<PathBuf, AppError> {
    fsio::config_path("broker.sock")
}

/// Сокет holder'а — фонового держателя cli-сессий на то время, когда GUI не
/// запущен (`sql-kai holder run`). Тот же протокол, что и у GUI-брокера.
pub fn holder_socket_path() -> Result<PathBuf, AppError> {
    fsio::config_path("holder.sock")
}

/// Бинд сокета holder'а: если по нему уже кто-то отвечает — занято (None,
/// живой holder обслужит клиентов сам), иначе протухший файл убирается и
/// поднимается свежий 0600-листенер. Гонка двух спавнеров решается самим
/// бинд-фактом: проигравший увидит живой сокет и выйдет.
#[cfg(unix)]
pub async fn bind_holder() -> Result<Option<UnixListener>, AppError> {
    let path = holder_socket_path()?;
    if UnixStream::connect(&path).await.is_ok() {
        return Ok(None);
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        // Как и у GUI-брокера: не ужали права — не поднимаемся, через сокет
        // выполняется SQL под разлоченным vault.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(Some(listener))
}

/// Best-effort «погасни» в сокет holder'а. Vault lock означает «ничего не
/// остаётся подключенным» — включая фоновый держатель cli-сессий.
#[cfg(unix)]
pub async fn shutdown_holder() {
    let Ok(path) = holder_socket_path() else { return };
    let Ok(stream) = UnixStream::connect(&path).await else { return };
    let (read, mut write) = stream.into_split();
    if write
        .write_all(b"{\"id\":1,\"method\":\"shutdown\",\"params\":null}\n")
        .await
        .is_err()
    {
        return;
    }
    // дождаться ответа (не дольше секунды), чтобы holder успел принять запрос
    let mut lines = BufReader::new(read).lines();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), lines.next_line()).await;
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
    /// выход sql-kai). `write` временно снимает session-wide read-only.
    Query {
        #[serde(rename = "profileId")]
        profile_id: String,
        sql: String,
        #[serde(default = "default_max_rows", rename = "maxRows")]
        max_rows: usize,
        #[serde(default)]
        write: bool,
        /// Вернуть и типы колонок (Parse) — для типизированного --json в sql-kai.
        #[serde(default, rename = "withTypes")]
        with_types: bool,
    },
    /// Отменить запрос, бегущий на cli-сессии профиля.
    Cancel {
        #[serde(rename = "profileId")]
        profile_id: String,
    },
    /// sql-kai изменил состав профилей (discover/rm) — GUI перечитывает список.
    ProfilesChanged,
    /// Погасить сервер: закрыть сессии и выйти (holder). GUI-брокер отвергает.
    Shutdown,
    /// DDL таблицы силами cli-сессии (MCP-tool `ddl` у `sql-kai mcp`).
    Ddl {
        #[serde(rename = "profileId")]
        profile_id: String,
        schema: String,
        table: String,
    },
    /// Открыть в GUI вкладку таблицы (MCP-tool `open_table`).
    OpenTable {
        #[serde(rename = "profileId")]
        profile_id: String,
        schema: String,
        table: String,
    },
    /// Открыть в GUI вкладку запроса с готовым SQL, не выполняя его
    /// (MCP-tool `open_query`).
    OpenQuery {
        #[serde(rename = "profileId")]
        profile_id: String,
        sql: String,
    },
    /// Что пользователь сейчас видит и выделил в GUI: активная вкладка,
    /// фильтр, выделенные строки/колонки/ячейки (MCP-tool `selection`).
    GuiSelection {
        #[serde(rename = "profileId")]
        profile_id: String,
    },
}

/// Что открыть в интерфейсе по просьбе MCP-клиента (методы open_*).
/// Сериализация — готовый payload события `agent://open` для webview.
#[derive(Serialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum GuiOpen {
    Table {
        profile_id: String,
        schema: String,
        table: String,
    },
    Query {
        profile_id: String,
        sql: String,
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
    /// по запросу sql-kai.
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

/// Колонки одного стейтмента как (имя, oid типа) — sql-kai восстанавливает
/// `Type::from_oid` на своей стороне.
pub type WireColumnTypes = Vec<Option<Vec<(String, u32)>>>;

// --- State ----------------------------------------------------------------------

pub struct CliEntry {
    pub session: db::Session,
    /// Сериализует запросы к одной сессии (двум sql-kai одновременно нельзя).
    busy: tokio::sync::Mutex<()>,
    last_used: Mutex<Instant>,
}

/// Владелец cli-сессий. std-Mutex, чтобы состав можно было чистить из
/// синхронных мест хоста (vault_lock); сами запросы держат только `busy`.
#[derive(Default)]
pub struct BrokerState {
    cli: Mutex<HashMap<String, Arc<CliEntry>>>,
    /// Момент последнего запроса по сокету (любого). Holder по нему решает,
    /// что он никому не нужен, и самозавершается; GUI-брокер не читает.
    last_activity: Mutex<Option<Instant>>,
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
                let ttl = match TxStatus::from_u8(e.session.tx.load(Ordering::Relaxed)) {
                    TxStatus::Idle => CLI_IDLE_TTL_SEC,
                    _ => CLI_TX_IDLE_TTL_SEC,
                };
                let live = !e.session.client.is_closed()
                    && e.last_used.lock().unwrap().elapsed().as_secs() < ttl;
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

    /// Отметить активность по сокету (см. `last_activity`).
    pub fn touch(&self) {
        *self.last_activity.lock().unwrap() = Some(Instant::now());
    }

    /// Нет сессий и нет запросов дольше `linger_sec` — holder'у пора выходить.
    /// Без единого запроса (None) простоем считается "бесконечность": holder
    /// вызывает touch() на старте, так что None здесь не встречается.
    pub fn is_idle(&self, linger_sec: u64) -> bool {
        if !self.cli.lock().unwrap().is_empty() {
            return false;
        }
        self.last_activity
            .lock()
            .unwrap()
            .map(|t| t.elapsed().as_secs() >= linger_sec)
            .unwrap_or(true)
    }

    fn get_live(&self, profile_id: &str) -> Option<Arc<CliEntry>> {
        let map = self.cli.lock().unwrap();
        map.get(profile_id)
            .filter(|e| !e.session.client.is_closed())
            .cloned()
    }

    fn remove_entry(&self, profile_id: &str) {
        self.cli.lock().unwrap().remove(profile_id);
    }
}

/// Future-хук «спроси GUI»: profile_id → payload ответа webview (или текст
/// ошибки). Ответ приезжает асинхронно (event → invoke), отсюда future.
pub type GuiSelectionHook = Box<
    dyn Fn(
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
        > + Send
        + Sync,
>;

/// Хост-специфика: как посмотреть GUI-сессии и как сообщить интерфейсу об
/// изменениях. В Tauri это AppState-снапшот и emit события; в будущем демоне —
/// пустой список и no-op.
pub struct BrokerHooks {
    pub gui_sessions: Box<dyn Fn() -> Vec<BrokerSessionInfo> + Send + Sync>,
    pub changed: Box<dyn Fn() + Send + Sync>,
    /// sql-kai сообщил об изменении профилей — интерфейс перечитывает список.
    pub profiles_changed: Box<dyn Fn() + Send + Sync>,
    /// Обработчик `shutdown`: holder закрывает сессии и завершается.
    /// None (GUI-брокер) — метод отвергается: приложение так не гасят.
    pub shutdown: Option<Box<dyn Fn() + Send + Sync>>,
    /// Открыть вкладку в GUI (методы open_table/open_query от MCP-tools).
    /// None (holder) — метод отвергается: интерфейса нет.
    pub open_in_gui: Option<Box<dyn Fn(GuiOpen) + Send + Sync>>,
    /// Текущая вкладка/выделение в GUI (метод gui_selection от MCP-tool
    /// `selection`). None (holder) — метод отвергается: интерфейса нет.
    pub gui_selection: Option<GuiSelectionHook>,
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
    /// SQLSTATE серверной ошибки (например 25006 read-only) — sql-kai по нему
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
    state.touch();
    match method {
        Method::Hello { client_version } => {
            if !client_version.is_empty() && client_version != env!("CARGO_PKG_VERSION") {
                logging::log(
                    "broker",
                    &format!(
                        "hello from sql-kai {client_version} (gui {})",
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
        Method::ProfilesChanged => {
            (hooks.profiles_changed)();
            Ok(json!({}))
        }
        Method::Shutdown => match &hooks.shutdown {
            Some(f) => {
                f();
                Ok(json!({}))
            }
            None => Err(method_err("unsupported", "этот сервер нельзя погасить по сокету")),
        },
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
        Method::Ddl { profile_id, schema, table } => {
            if !vault::is_unlocked() {
                return Err(method_err("vault_locked", "vault заблокирован в GUI"));
            }
            let entry = get_or_open(state, hooks, &profile_id)
                .await
                .map_err(|e| method_err("connect", e.to_string()))?;
            let _busy = entry.busy.lock().await;
            *entry.last_used.lock().unwrap() = Instant::now();
            let ddl = db::table_ddl(&entry.session.client, &schema, &table)
                .await
                .map_err(|e| MethodError {
                    code: "query",
                    message: e.to_string(),
                    sqlstate: e.sqlstate().map(str::to_string),
                })?;
            Ok(json!({ "ddl": ddl }))
        }
        Method::OpenTable { profile_id, schema, table } => match &hooks.open_in_gui {
            Some(open) => {
                open(GuiOpen::Table { profile_id, schema, table });
                Ok(json!({}))
            }
            None => Err(method_err("unsupported", "GUI не запущен — вкладку открыть некому")),
        },
        Method::OpenQuery { profile_id, sql } => match &hooks.open_in_gui {
            Some(open) => {
                open(GuiOpen::Query { profile_id, sql });
                Ok(json!({}))
            }
            None => Err(method_err("unsupported", "GUI не запущен — вкладку открыть некому")),
        },
        Method::GuiSelection { profile_id } => match &hooks.gui_selection {
            // таймаут — внутри хука (webview может не ответить); брокер
            // просто ждёт future
            Some(ask) => ask(profile_id).await.map_err(|e| method_err("gui", e)),
            None => Err(method_err(
                "unsupported",
                "GUI не запущен — состояние интерфейса спросить не у кого",
            )),
        },
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
        // sql-kai по этому коду откатывается на автономный путь со своей
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
                state.remove_entry(profile_id);
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
    let profile = store::profile_by_id(profile_id)?;
    let connected = db::connect(
        &profile,
        db::ConnectOptions {
            ssh_mux_ttl: Some(crate::tunnel::DEFAULT_MUX_TTL),
            ..Default::default()
        },
    )
    .await?;
    let mut session = connected.session;
    let closed_rx = session.closed_rx.take();
    let _ = db::execute(&session.client, "SET default_transaction_read_only = on", 1).await;
    let entry = Arc::new(CliEntry {
        session,
        busy: tokio::sync::Mutex::new(()),
        last_used: Mutex::new(Instant::now()),
    });
    let (winner, ours_won) = {
        let mut map = state.cli.lock().unwrap();
        // гонка двух sql-kai: если параллельный открыватель успел раньше и его
        // сессия жива — наша лишняя, отдаём его
        match map.get(profile_id) {
            Some(existing) if !existing.session.client.is_closed() => (existing.clone(), false),
            _ => {
                map.insert(profile_id.to_string(), entry.clone());
                (entry, true)
            }
        }
    };
    if ours_won {
        if let Some(rx) = closed_rx {
            watch_cli_session_closed(state, hooks, profile_id, &winner, rx);
        }
    }
    logging::log(
        "broker",
        &format!("\"{}\": cli-сессия открыта по запросу sql-kai", profile.name),
    );
    // отметка «подключались по cli» + profiles_changed, чтобы лаунчер
    // перечитал профили и обновил "last connected" сразу
    let _ = store::record_last_connected(profile_id, store::ConnectVia::Cli);
    (hooks.profiles_changed)();
    (hooks.changed)();
    Ok(winner)
}

/// Смерть провода cli-сессии (см. Session::closed_rx) — выкинуть её сразу и
/// дёрнуть `changed`, чтобы бейдж в GUI погас мгновенно, а не через sweep
/// (до 60 с) или следующий запрос sql-kai. Держим Weak: сильная ссылка не дала бы
/// vault_lock/clear() до конца снести сессию (её туннель) — Drop сработал бы
/// только после этого watcher'а, который сам ждёт Drop.
#[cfg(unix)]
fn watch_cli_session_closed(
    state: &Arc<BrokerState>,
    hooks: &Arc<BrokerHooks>,
    profile_id: &str,
    entry: &Arc<CliEntry>,
    rx: tokio::sync::oneshot::Receiver<String>,
) {
    let state = state.clone();
    let hooks = hooks.clone();
    let profile_id = profile_id.to_string();
    let ours = Arc::downgrade(entry);
    tokio::spawn(async move {
        let Ok(_reason) = rx.await else {
            return; // сессию закрыли штатно (clear/sweep) — уже учтено
        };
        let removed = {
            let mut map = state.cli.lock().unwrap();
            // убираем только СВОЮ запись: место могла успеть занять свежая
            match (map.get(&profile_id), ours.upgrade()) {
                (Some(cur), Some(ours)) if Arc::ptr_eq(cur, &ours) => map.remove(&profile_id),
                _ => None,
            }
        };
        if removed.is_some() {
            drop(removed); // teardown туннеля — вне лока
            logging::log(
                "broker",
                &format!("cli-сессия профиля {profile_id} закрыта: соединение умерло"),
            );
            (hooks.changed)();
        }
    });
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
            profiles_changed: Box::new(|| {}),
            shutdown: None,
            open_in_gui: None,
            gui_selection: None,
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

    /// profiles_changed дёргает хук и отвечает без ошибки — контракт для
    /// notify_profiles_changed() в sql-kai (discover/rm).
    #[cfg(unix)]
    #[tokio::test]
    async fn profiles_changed_fires_hook() {
        let path = std::env::temp_dir()
            .join(format!("kai-broker-test-pc-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f = fired.clone();
        let hooks = Arc::new(BrokerHooks {
            gui_sessions: Box::new(Vec::new),
            changed: Box::new(|| {}),
            profiles_changed: Box::new(move || f.store(true, Ordering::Relaxed)),
            shutdown: None,
            open_in_gui: None,
            gui_selection: None,
        });
        tokio::spawn(serve(listener, Arc::new(BrokerState::default()), hooks));

        let stream = UnixStream::connect(&path).await.unwrap();
        let (r, mut w) = stream.into_split();
        let mut lines = BufReader::new(r).lines();
        w.write_all(b"{\"id\":1,\"method\":\"profiles_changed\",\"params\":null}\n")
            .await
            .unwrap();
        let v: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert!(v.get("error").is_none(), "unexpected reply: {v}");
        assert!(fired.load(Ordering::Relaxed));
        let _ = std::fs::remove_file(&path);
    }

    /// shutdown: с хуком (holder) — ok и хук дёрнут; без хука (GUI) — ошибка
    /// unsupported. Контракт для vault_lock / sql-kai holder stop.
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_dispatch_by_hook() {
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f = fired.clone();
        let with_hook = Arc::new(BrokerHooks {
            gui_sessions: Box::new(Vec::new),
            changed: Box::new(|| {}),
            profiles_changed: Box::new(|| {}),
            shutdown: Some(Box::new(move || f.store(true, Ordering::Relaxed))),
            open_in_gui: None,
            gui_selection: None,
        });
        let state = Arc::new(BrokerState::default());
        let r = dispatch(Method::Shutdown, &state, &with_hook).await;
        assert!(r.is_ok());
        assert!(fired.load(Ordering::Relaxed));

        let without_hook = Arc::new(BrokerHooks {
            gui_sessions: Box::new(Vec::new),
            changed: Box::new(|| {}),
            profiles_changed: Box::new(|| {}),
            shutdown: None,
            open_in_gui: None,
            gui_selection: None,
        });
        let r = dispatch(Method::Shutdown, &state, &without_hook).await;
        assert!(matches!(r, Err(e) if e.code == "unsupported"));
    }

    /// open_table/open_query: с хуком (GUI) — ok и payload доходит; без хука
    /// (holder) — unsupported. Контракт для MCP-tools sql-kai.
    #[cfg(unix)]
    #[tokio::test]
    async fn open_in_gui_dispatch_by_hook() {
        let opened = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = opened.clone();
        let with_hook = Arc::new(BrokerHooks {
            gui_sessions: Box::new(Vec::new),
            changed: Box::new(|| {}),
            profiles_changed: Box::new(|| {}),
            shutdown: None,
            open_in_gui: Some(Box::new(move |open| {
                sink.lock()
                    .unwrap()
                    .push(serde_json::to_string(&open).unwrap());
            })),
            gui_selection: None,
        });
        let state = Arc::new(BrokerState::default());
        let r = dispatch(
            Method::OpenTable {
                profile_id: "p1".into(),
                schema: "public".into(),
                table: "users".into(),
            },
            &state,
            &with_hook,
        )
        .await;
        assert!(r.is_ok());
        let payloads = opened.lock().unwrap().clone();
        assert_eq!(payloads.len(), 1);
        let v: Value = serde_json::from_str(&payloads[0]).unwrap();
        assert_eq!(v["kind"], "table");
        assert_eq!(v["profileId"], "p1");
        assert_eq!(v["table"], "users");

        let without_hook = Arc::new(BrokerHooks {
            gui_sessions: Box::new(Vec::new),
            changed: Box::new(|| {}),
            profiles_changed: Box::new(|| {}),
            shutdown: None,
            open_in_gui: None,
            gui_selection: None,
        });
        let r = dispatch(
            Method::OpenQuery { profile_id: "p1".into(), sql: "SELECT 1".into() },
            &state,
            &without_hook,
        )
        .await;
        assert!(matches!(r, Err(e) if e.code == "unsupported"));
    }

    /// gui_selection: с хуком (GUI) — payload webview уходит как result;
    /// без хука (holder) — unsupported. Контракт для MCP-tool `selection`.
    #[cfg(unix)]
    #[tokio::test]
    async fn gui_selection_dispatch_by_hook() {
        let with_hook = Arc::new(BrokerHooks {
            gui_sessions: Box::new(Vec::new),
            changed: Box::new(|| {}),
            profiles_changed: Box::new(|| {}),
            shutdown: None,
            open_in_gui: None,
            gui_selection: Some(Box::new(|profile_id| {
                Box::pin(async move {
                    Ok(json!({ "profileId": profile_id, "selection": { "kind": "none" } }))
                })
            })),
        });
        let state = Arc::new(BrokerState::default());
        let r = dispatch(
            Method::GuiSelection { profile_id: "p1".into() },
            &state,
            &with_hook,
        )
        .await;
        let Ok(v) = r else {
            panic!("gui_selection with hook should succeed");
        };
        assert_eq!(v["profileId"], "p1");

        let without_hook = Arc::new(BrokerHooks {
            gui_sessions: Box::new(Vec::new),
            changed: Box::new(|| {}),
            profiles_changed: Box::new(|| {}),
            shutdown: None,
            open_in_gui: None,
            gui_selection: None,
        });
        let r = dispatch(
            Method::GuiSelection { profile_id: "p1".into() },
            &state,
            &without_hook,
        )
        .await;
        assert!(matches!(r, Err(e) if e.code == "unsupported"));
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
