//! Wire-типы протокола брокера: запрос/методы, ответы и их полезные нагрузки.
//! Сериализация — контракт с sql-kai-cli (broker_client.rs), менять только
//! вместе с PROTOCOL_VERSION.

use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};

use crate::db::{self, TxStatus};

pub const PROTOCOL_VERSION: u32 = 1;

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

pub(super) fn default_max_rows() -> usize {
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
