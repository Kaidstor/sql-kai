//! Клиент брокера GUI: если приложение запущено (живой unix-сокет), kai
//! выполняет запросы его силами — vault уже разблокирован, туннель уже
//! поднят, cli-сессия переживает выход kai и переиспользуется следующим
//! запуском. Любая транспортная проблема — тихий откат на автономный путь.

use serde_json::{json, Value};
use sql_kai_lib::broker::{self, BrokerSessionInfo, HelloReply, WireColumnTypes};
use sql_kai_lib::db::ExecResult;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub struct BrokerClient {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    next_id: u64,
    pub hello: HelloReply,
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

/// Живой брокер или None (GUI не запущен / несовместимый протокол).
pub async fn connect() -> Option<BrokerClient> {
    let path = broker::socket_path().ok()?;
    let stream = UnixStream::connect(&path).await.ok()?;
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
