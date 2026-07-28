use serde::{ser::SerializeStruct, Serialize, Serializer};
use tokio_postgres::error::SqlState;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Msg(String),
    /// Команда получила id сессии, которой уже нет в карте (закрыта/забыта).
    #[error("session not found (already disconnected?)")]
    SessionGone,
    /// Живой клиент оказался мёртвым (туннель/сервер упал) — сессия выброшена.
    #[error("connection lost (tunnel or server dropped) — reconnect the profile")]
    ConnectionLost,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{}", format_pg_error(.0))]
    Pg(#[from] tokio_postgres::Error),
    /// Батч отвергнут read-only гейтом ещё до отправки на сервер (см.
    /// `db::execute_read_only`). Для потребителей это тот же класс, что
    /// SQLSTATE 25006: нужен write-доступ, а не другой SQL, — поэтому и код
    /// тот же ("read_only"), UI/CLI ветвятся одинаково на оба источника.
    #[error("{0}")]
    ReadOnlyRefused(String),
}

impl AppError {
    /// Машиночитаемый код для фронта и CLI — вместо разбора текста ошибки.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Msg(_) => "app",
            AppError::SessionGone => "session_gone",
            AppError::ConnectionLost => "connection_lost",
            AppError::Io(_) => "io",
            // Смерть провода (соединение закрыто / io-ошибка под запросом)
            // для UI равна потере сессии — лечится реконнектом, не правкой SQL.
            AppError::Pg(e) if is_wire_death(e) => "connection_lost",
            AppError::Pg(e) if e.as_db_error().is_some() => match self.sqlstate() {
                Some(s) if s == SqlState::READ_ONLY_SQL_TRANSACTION.code() => "read_only",
                _ => "db",
            },
            AppError::Pg(_) => "pg",
            AppError::ReadOnlyRefused(_) => "read_only",
        }
    }

    /// SQLSTATE ошибки сервера (например "25006"), если ошибка от него.
    pub fn sqlstate(&self) -> Option<&str> {
        match self {
            AppError::Pg(e) => e.as_db_error().map(|db| db.code().code()),
            _ => None,
        }
    }

    /// Сессия read-only (SQLSTATE 25006 или отказ гейта до отправки) —
    /// sql-kai подсказывает `--write`.
    pub fn is_read_only(&self) -> bool {
        matches!(self, AppError::ReadOnlyRefused(_))
            || self.sqlstate() == Some(SqlState::READ_ONLY_SQL_TRANSACTION.code())
    }
}

/// Ошибка tokio-postgres, означающая смерть соединения, а не плохой запрос:
/// «connection closed» или io-ошибка на проводе (broken pipe, reset, …).
fn is_wire_death(e: &tokio_postgres::Error) -> bool {
    use std::error::Error as _;
    e.is_closed() || e.source().is_some_and(|s| s.is::<std::io::Error>())
}

// На фронт уходит `{code, message}` — UI ветвится по коду (reconnect-баннер,
// hint про --write), а не по regex на тексте.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("code", self.code())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

fn format_pg_error(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        let mut s = format!("{}: {}", db.severity(), db.message());
        if let Some(d) = db.detail() {
            s.push_str(&format!("\nDETAIL: {d}"));
        }
        if let Some(h) = db.hint() {
            s.push_str(&format!("\nHINT: {h}"));
        }
        if let Some(tokio_postgres::error::ErrorPosition::Original(pos)) = db.position() {
            s.push_str(&format!("\nPOSITION: {pos}"));
        }
        s
    } else {
        e.to_string()
    }
}
