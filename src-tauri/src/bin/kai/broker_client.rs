//! Клиент серверов сессий kai — GUI-брокера и holder'а (общий протокол).
//! Если жив unix-сокет одного из них, kai выполняет запросы его силами:
//! vault уже разблокирован, туннель уже поднят, cli-сессия переживает выход
//! kai и переиспользуется следующим запуском. Когда GUI не запущен, holder
//! спавнится по требованию (см. connect_any). Любая транспортная проблема —
//! тихий откат на автономный путь.

use std::path::Path;

use serde_json::{json, Value};
use sql_kai_lib::broker::{self, BrokerSessionInfo, HelloReply, WireColumnTypes};
use sql_kai_lib::db::ExecResult;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::session;

/// Какой сервер сессий обслуживает это соединение.
#[derive(Clone, Copy, PartialEq)]
pub enum Via {
    /// Брокер запущенного GUI-приложения.
    Gui,
    /// Фоновый holder (`kai holder run`).
    Holder,
}

pub struct BrokerClient {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    next_id: u64,
    pub hello: HelloReply,
    pub via: Via,
}

pub struct BrokerQuery {
    pub exec: ExecResult,
    pub column_types: Option<WireColumnTypes>,
}

pub enum BrokerError {
    /// Транспорт/протокол умер — уходим на автономный путь.
    Transport(String),
    /// Vault в GUI заблокирован — kai разблокирует сам (автономный путь).
    VaultLocked,
    /// Сервер выполнил запрос и вернул ошибку (SQL и т.п.) — финальный ответ.
    /// `sqlstate` — код ошибки Postgres (например 25006 read-only), если есть.
    Query {
        message: String,
        sqlstate: Option<String>,
    },
}

impl std::fmt::Display for BrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrokerError::Transport(m) => write!(f, "broker: {m}"),
            BrokerError::VaultLocked => write!(f, "vault заблокирован в GUI"),
            BrokerError::Query { message, .. } => write!(f, "{message}"),
        }
    }
}

/// Живой брокер GUI или None (GUI не запущен / несовместимый протокол).
pub async fn connect() -> Option<BrokerClient> {
    connect_path(&broker::socket_path().ok()?, Via::Gui).await
}

/// Живой holder или None (не запущен / несовместимый протокол).
pub async fn connect_holder() -> Option<BrokerClient> {
    connect_path(&broker::holder_socket_path().ok()?, Via::Holder).await
}

/// Повторное соединение с тем же сервером (например, cancel параллельно
/// занятому запросом сокету).
pub async fn reconnect(via: Via) -> Option<BrokerClient> {
    match via {
        Via::Gui => connect().await,
        Via::Holder => connect_holder().await,
    }
}

/// Живой сервер сессий: GUI-брокер → holder → спавн holder'а. None — только
/// автономный путь (нет тихого доступа к vault или holder не поднялся).
pub async fn connect_any() -> Option<BrokerClient> {
    if let Some(b) = connect().await {
        return Some(b);
    }
    if let Some(b) = connect_holder().await {
        // Holder от прежнего бинаря (kai обновился) — гасим и поднимаем свежий.
        if b.hello.server_version == env!("CARGO_PKG_VERSION") {
            return Some(b);
        }
        broker::shutdown_holder().await;
        // Гашение асинхронное: holder сначала отвечает и лишь потом (~150 мс)
        // убирает сокет и выходит. Не дождавшись, свежий holder увидит живой
        // сокет и погибнет, а мы переподключимся к старому.
        if let Ok(path) = broker::holder_socket_path() {
            for _ in 0..20 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                if UnixStream::connect(&path).await.is_err() {
                    break;
                }
            }
        }
    }
    if !session::headless_unlock_possible() {
        return None; // holder всё равно не разлочит vault — не плодим процессы
    }
    spawn_holder().ok()?;
    // ждём, пока holder разлочит vault и займёт сокет (обычно < 300 мс)
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Some(b) = connect_holder().await {
            // Недобитый старый holder не возвращаем: свежий узнаём по версии.
            if b.hello.server_version == env!("CARGO_PKG_VERSION") {
                return Some(b);
            }
        }
    }
    None
}

/// Запускает holder тем же бинарём в фоне: своя process group и без stdio,
/// чтобы он пережил выход kai и не ловил сигналы его терминала.
fn spawn_holder() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("holder")
        .arg("run")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_| ())
}

async fn connect_path(path: &Path, via: Via) -> Option<BrokerClient> {
    let stream = UnixStream::connect(path).await.ok()?;
    let (read, writer) = stream.into_split();
    let mut b = BrokerClient {
        reader: BufReader::new(read),
        writer,
        next_id: 0,
        hello: HelloReply {
            protocol: 0,
            server_version: String::new(),
            vault_unlocked: false,
        },
        via,
    };
    let hello: HelloReply = serde_json::from_value(
        b.request(
            "hello",
            json!({ "client_version": env!("CARGO_PKG_VERSION") }),
        )
        .await
        .ok()?,
    )
    .ok()?;
    if hello.protocol != broker::PROTOCOL_VERSION {
        return None;
    }
    b.hello = hello;
    Some(b)
}

impl BrokerClient {
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, BrokerError> {
        self.next_id += 1;
        let mut line = serde_json::to_vec(&json!({
            "id": self.next_id,
            "method": method,
            "params": params,
        }))
        .map_err(|e| BrokerError::Transport(e.to_string()))?;
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .await
            .map_err(|e| BrokerError::Transport(e.to_string()))?;
        let mut reply = String::new();
        let n = self
            .reader
            .read_line(&mut reply)
            .await
            .map_err(|e| BrokerError::Transport(e.to_string()))?;
        if n == 0 {
            return Err(BrokerError::Transport("соединение закрыто".into()));
        }
        let v: Value = serde_json::from_str(&reply)
            .map_err(|e| BrokerError::Transport(format!("bad reply: {e}")))?;
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            let code = v.get("code").and_then(Value::as_str).unwrap_or("");
            return Err(match code {
                "vault_locked" => BrokerError::VaultLocked,
                "query" => BrokerError::Query {
                    message: err.to_string(),
                    sqlstate: v
                        .get("sqlstate")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
                // connect/cancel/no_session/protocol — считаем транспортными:
                // автономный путь либо решит проблему, либо покажет свою ошибку
                _ => BrokerError::Transport(err.to_string()),
            });
        }
        v.get("result")
            .cloned()
            .ok_or_else(|| BrokerError::Transport("ответ без result".into()))
    }

    pub async fn sessions(&mut self) -> Result<Vec<BrokerSessionInfo>, BrokerError> {
        // unit-вариант метода: params должен быть null, не {}
        let v = self.request("sessions", Value::Null).await?;
        serde_json::from_value(v).map_err(|e| BrokerError::Transport(e.to_string()))
    }

    pub async fn query(
        &mut self,
        profile_id: &str,
        sql: &str,
        max_rows: usize,
        write: bool,
        with_types: bool,
    ) -> Result<BrokerQuery, BrokerError> {
        let v = self
            .request(
                "query",
                json!({
                    "profileId": profile_id,
                    "sql": sql,
                    "maxRows": max_rows,
                    "write": write,
                    "withTypes": with_types,
                }),
            )
            .await?;
        let exec: ExecResult = serde_json::from_value(
            v.get("exec")
                .cloned()
                .ok_or_else(|| BrokerError::Transport("ответ без exec".into()))?,
        )
        .map_err(|e| BrokerError::Transport(e.to_string()))?;
        let column_types: Option<WireColumnTypes> = v
            .get("columnTypes")
            .and_then(|ct| serde_json::from_value(ct.clone()).ok());
        Ok(BrokerQuery { exec, column_types })
    }

    pub async fn cancel(&mut self, profile_id: &str) -> Result<(), BrokerError> {
        self.request("cancel", json!({ "profileId": profile_id }))
            .await
            .map(|_| ())
    }
}

/// Сообщить запущенному GUI, что состав профилей изменился (discover/rm),
/// чтобы тот перечитал список. Best-effort: GUI не запущен или старый —
/// молча ничего не делаем.
pub async fn notify_profiles_changed() {
    if let Some(mut b) = connect().await {
        let _ = b.request("profiles_changed", Value::Null).await;
    }
}
