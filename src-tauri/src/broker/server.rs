//! Цикл unix-сокета и диспетчер методов брокера (GUI-процесс и holder).
//! Модуль целиком unix-only — на прочих платформах брокера нет.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio_postgres::NoTls;

use crate::db::{self, TxStatus};
use crate::error::AppError;
use crate::logging;
use crate::store::{self, Profile};
use crate::vault;

use super::protocol::{
    HelloReply, Method, QueryParams, Request, WireColumnTypes, PROTOCOL_VERSION,
};
use super::state::{BrokerState, CliEntry};
use super::{BrokerHooks, GuiOpen};

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

struct MethodError {
    code: &'static str,
    message: String,
    /// SQLSTATE серверной ошибки (например 25006 read-only) — sql-kai по нему
    /// показывает hint, не разбирая текст. Старые клиенты поле игнорируют.
    sqlstate: Option<String>,
}

fn method_err(code: &'static str, message: impl Into<String>) -> MethodError {
    MethodError {
        code,
        message: message.into(),
        sqlstate: None,
    }
}

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
        Method::Query(q) => do_query(state, hooks, &q).await,
        Method::ProfilesChanged => {
            (hooks.profiles_changed)();
            Ok(json!({}))
        }
        Method::Shutdown => match &hooks.shutdown {
            Some(f) => {
                f();
                Ok(json!({}))
            }
            None => Err(method_err(
                "unsupported",
                "этот сервер нельзя погасить по сокету",
            )),
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
        Method::Ddl {
            profile_id,
            schema,
            table,
        } => {
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
        Method::OpenTable {
            profile_id,
            schema,
            table,
        } => match &hooks.open_in_gui {
            Some(open) => {
                open(GuiOpen::Table {
                    profile_id,
                    schema,
                    table,
                });
                Ok(json!({}))
            }
            None => Err(method_err(
                "unsupported",
                "GUI не запущен — вкладку открыть некому",
            )),
        },
        Method::OpenQuery { profile_id, sql } => match &hooks.open_in_gui {
            Some(open) => {
                open(GuiOpen::Query { profile_id, sql });
                Ok(json!({}))
            }
            None => Err(method_err(
                "unsupported",
                "GUI не запущен — вкладку открыть некому",
            )),
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

/// `SQL_KAI_ALLOW_PROD_WRITE` в окружении *сервера* — разбор общий с cli
/// (`session::prod`), см. [`crate::prod`].
fn env_allows_prod_write(profile: &Profile) -> bool {
    crate::prod::env_allows(crate::envvar::ALLOW_PROD_WRITE, &profile.name, &profile.id)
}

/// Прод-барьер на стороне сервера. До этого он жил только в cli
/// (`session::prod`), то есть обходился любым другим клиентом сокета — старым
/// sql-kai, который про барьер не знает, или парой строк на питоне: сокет
/// принадлежит пользователю, а секреты уже расшифрованы здесь.
///
/// Спросить человека сервер не может (он внутри GUI, tty у него нет), поэтому
/// признаёт два источника разрешения:
///   * `SQL_KAI_ALLOW_PROD_WRITE` в своём окружении — настоящая граница: env
///     запущенного процесса чужой процесс не поменяет;
///   * `prodWriteAuthorized` от клиента — блокировка: подтверждает, что клиент
///     барьер реализует и человек через него прошёл. Процесс того же
///     пользователя это поле подделает, и защитой от него служит не сокет, а
///     права на файлы профилей; зато случайная запись мимо барьера
///     (обновлённый GUI + старый cli) теперь невозможна.
fn guard_prod_write(profile_id: &str, client_authorized: bool) -> Result<(), MethodError> {
    // Профиль не читается — считаем прод: чего не видим, то не считаем безопасным.
    let profile = store::profile_by_id(profile_id)
        .map_err(|e| method_err("prod_write", format!("профиль не прочитан: {e}")))?;
    if !profile.production {
        return Ok(());
    }
    if env_allows_prod_write(&profile) || client_authorized {
        logging::log(
            "broker",
            &format!(
                "prod write allowed for \"{}\" ({}@{}:{}/{}) — {}",
                profile.name,
                profile.user,
                profile.host,
                profile.port,
                profile.database,
                if client_authorized {
                    "client-authorized"
                } else {
                    "env allowlist"
                }
            ),
        );
        return Ok(());
    }
    logging::log(
        "broker",
        &format!(
            "prod write refused for \"{}\": no authorization",
            profile.name
        ),
    );
    Err(MethodError {
        code: "prod_write",
        message: format!(
            "запись в production-профиль '{}' заблокирована сервером сессий: запрос пришёл \
             без подтверждения прод-барьера. Обнови sql-kai (старый клиент барьер не \
             проходит) либо задай SQL_KAI_ALLOW_PROD_WRITE={} в окружении процесса, \
             который держит сессии (GUI или holder).",
            profile.name, profile.name
        ),
        sqlstate: None,
    })
}

async fn do_query(
    state: &Arc<BrokerState>,
    hooks: &Arc<BrokerHooks>,
    q: &QueryParams,
) -> Result<Value, MethodError> {
    let (profile_id, sql) = (q.profile_id.as_str(), q.sql.as_str());
    let (max_rows, write, with_types) = (q.max_rows, q.write, q.with_types);
    if !vault::is_unlocked() {
        // sql-kai по этому коду откатывается на автономный путь со своей
        // цепочкой разблокировки
        return Err(method_err("vault_locked", "vault заблокирован в GUI"));
    }
    if write {
        guard_prod_write(profile_id, q.prod_write_authorized)?;
    }
    let entry = get_or_open(state, hooks, profile_id)
        .await
        .map_err(|e| method_err("connect", e.to_string()))?;
    let _busy = entry.busy.lock().await;
    *entry.last_used.lock().unwrap() = Instant::now();

    let client = &entry.session.client;
    let executor = db::QueryExecutor::new(client, &entry.session.tx);
    // Read-only держится на ЯВНОЙ read-only транзакции вокруг каждого читающего
    // батча, а не на default_transaction_read_only: тот USERSET, и батч снимал
    // его сам первым же стейтментом. GUC остаётся как второй слой (сессия
    // открывается с ним, --write снимает его на время запроса), но границей
    // теперь служит блок BEGIN READ ONLY — внутри него Postgres не даёт ни
    // писать, ни повысить права (25001).
    //
    // Отдельно трекаем режим открытой транзакции: если --write оставил
    // read-write транзакцию (BEGIN без COMMIT), читающий вызов продолжил бы её
    // и смог писать, поэтому внутрь такой транзакции его не пускаем.
    let before = executor.status();
    let before_write = entry.session.tx_write.load(Ordering::Relaxed);
    // Читающий вызов идёт внутри явной read-only транзакции: снять с себя
    // read-only внутри неё Postgres не даёт (25001), поэтому единственный обход
    // — выйти из блока, и он отсекается здесь. Исключение — батч из одного лишь
    // COMMIT/ROLLBACK: это штатный способ закрыть транзакцию, которую оставил
    // открытой предыдущий --write, и писать он не умеет.
    let tx_control_only = db::is_tx_control_only(sql);
    let read_only_tx = !write && !tx_control_only;
    if read_only_tx && db::escapes_read_only_tx(sql) {
        return Err(MethodError {
            code: "read_only_tx",
            message: "read-only сессия: батч вышел бы из read-only транзакции, в которой \
                      выполняется, или снял бы с неё read-only (COMMIT/ROLLBACK/END/ABORT/\
                      PREPARE TRANSACTION/DISCARD/SET TRANSACTION/SET …transaction_read_only, \
                      включая форму set_config()). Повтори с --write, если он действительно \
                      должен менять данные."
                .into(),
            sqlstate: Some("25006".into()),
        });
    }
    if !write && before != TxStatus::Idle && before_write && !tx_control_only {
        return Err(MethodError {
            code: "read_only_tx",
            message: "открыта read-write транзакция (её начал вызов с --write). \
                      Заверши её COMMIT/ROLLBACK или повтори запрос с --write."
                .into(),
            // read_only_sql_transaction — ближайший по смыслу SQLSTATE
            sqlstate: Some("25006".into()),
        });
    }

    // Батч из одного COMMIT идёт мимо read-only обёртки как штатный способ
    // закрыть транзакцию, которую оставил открытой предыдущий --write. Но если
    // та транзакция read-write, COMMIT фиксирует чужую запись — на production
    // это ровно та операция, ради которой барьер и стоит, а прийти сюда может
    // кто угодно, в том числе MCP-агент с write:false. ROLLBACK не трогаем:
    // выбросить незакоммиченное можно всегда.
    if !write && tx_control_only && before_write && db::commits_tx(sql) {
        guard_prod_write(profile_id, q.prod_write_authorized)?;
    }

    if write {
        // Ошибку снятия read-only больше не игнорируем: если флаг не снят,
        // выполнять write-запрос нельзя — вернём ошибку вместо тихой записи в
        // (казалось бы) read-only сессии.
        db::execute(client, "SET default_transaction_read_only = off", 1)
            .await
            .map_err(|e| {
                method_err(
                    "write_setup",
                    format!("не удалось включить режим записи: {e}"),
                )
            })?;
    }

    let result = if read_only_tx {
        executor
            .execute_read_only(sql, max_rows.clamp(1, 100_000))
            .await
    } else {
        executor.execute(sql, max_rows.clamp(1, 100_000)).await
    };
    let after = executor.status();
    // Открытая транзакция read-write, если её открыл (или продолжил) --write.
    let after_write = after != TxStatus::Idle && (write || before_write);
    entry.session.tx_write.store(after_write, Ordering::Relaxed);

    // Возвращаем read-only default, как только вернулись в чистый idle — тогда
    // следующая неявная транзакция снова read-only. Внутри открытой/aborted
    // транзакции SET бессмыслен (упал бы с 25P02), там охраняет gate выше.
    if after == TxStatus::Idle && (write || before_write) {
        if let Err(e) = db::execute(client, "SET default_transaction_read_only = on", 1).await {
            // Не удалось восстановить read-only на живой сессии — не рискуем:
            // выкидываем её, следующий вызов откроет свежую (снова read-only).
            logging::log(
                "broker",
                &format!("\"{profile_id}\": failed to re-arm read-only ({e}); dropping session"),
            );
            state.remove_entry(profile_id);
            (hooks.changed)();
        }
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
async fn get_or_open(
    state: &Arc<BrokerState>,
    hooks: &Arc<BrokerHooks>,
    profile_id: &str,
) -> Result<Arc<CliEntry>, AppError> {
    if let Some(entry) = state.get_live(profile_id) {
        return Ok(entry);
    }
    let profile = store::profile_by_id(profile_id)?;
    // Профиль заявляет пароль, а в нашей памяти его нет — значит секрет положил
    // другой процесс уже после того, как мы разблокировали vault (типичный
    // случай: `sql-kai fork` завёл профиль, и первый же запрос к нему пришёл
    // сюда). Перечитываем секреты, иначе ушли бы коннектиться без пароля и
    // получили бы от tokio-postgres невнятное "invalid configuration".
    if profile.has_password && vault::get_secret(&profile.id).is_none() {
        if let Err(e) = vault::refresh_secrets() {
            logging::log(
                "broker",
                &format!("\"{profile_id}\": failed to refresh vault secrets ({e})"),
            );
        }
    }
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
    // Не встал read-only default — сессию не отдаём: иначе она молча
    // окажется read-write (тот же принцип, что у write-SET'а в do_query).
    db::execute(&session.client, "SET default_transaction_read_only = on", 1)
        .await
        .map_err(|e| AppError::Msg(format!("не удалось перевести cli-сессию в read-only: {e}")))?;
    let entry = Arc::new(CliEntry::new(session));
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
        &format!(
            "\"{}\": cli session opened on sql-kai request",
            profile.name
        ),
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
                &format!("cli session of profile {profile_id} closed: the connection died"),
            );
            (hooks.changed)();
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Живой round-trip через настоящий unix-сокет: hello и sessions (пустое
    /// состояние, без БД). Также фиксирует, что params: null и отсутствующий
    /// params валидны для unit-вариантов.
    #[tokio::test]
    async fn hello_and_sessions_over_socket() {
        let path =
            std::env::temp_dir().join(format!("kai-broker-test-{}.sock", std::process::id()));
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
        let v: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
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
    #[tokio::test]
    async fn profiles_changed_fires_hook() {
        let path =
            std::env::temp_dir().join(format!("kai-broker-test-pc-{}.sock", std::process::id()));
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
        let v: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert!(v.get("error").is_none(), "unexpected reply: {v}");
        assert!(fired.load(Ordering::Relaxed));
        let _ = std::fs::remove_file(&path);
    }

    /// shutdown: с хуком (holder) — ok и хук дёрнут; без хука (GUI) — ошибка
    /// unsupported. Контракт для vault_lock / sql-kai holder stop.
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
            Method::OpenQuery {
                profile_id: "p1".into(),
                sql: "SELECT 1".into(),
            },
            &state,
            &without_hook,
        )
        .await;
        assert!(matches!(r, Err(e) if e.code == "unsupported"));
    }

    /// gui_selection: с хуком (GUI) — payload webview уходит как result;
    /// без хука (holder) — unsupported. Контракт для MCP-tool `selection`.
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
            Method::GuiSelection {
                profile_id: "p1".into(),
            },
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
            Method::GuiSelection {
                profile_id: "p1".into(),
            },
            &state,
            &without_hook,
        )
        .await;
        assert!(matches!(r, Err(e) if e.code == "unsupported"));
    }
}
