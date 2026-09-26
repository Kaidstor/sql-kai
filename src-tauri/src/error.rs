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
    /// Искомого нет: профиль, сохранённый запрос.
    #[error("{0}")]
    NotFound(String),
    /// Аргументы не складываются в команду: алиас подходит нескольким
    /// профилям, нет SQL, нет TTY для подтверждения без флага.
    #[error("{0}")]
    Usage(String),
    /// Нечем аутентифицироваться: vault заперт, мастер-пароль не подошёл,
    /// пароля БД нет в указанном источнике.
    #[error("{0}")]
    Auth(String),
    /// Не настроено то, без чего команда не идёт: vault не создан, нет sec,
    /// битые TLS-файлы профиля.
    #[error("{0}")]
    Config(String),
    /// До сервера не достучались: ssh-туннель не поднялся.
    #[error("{0}")]
    Network(String),
    /// Сервер не ответил в срок, пока поднималось подключение.
    #[error("{0}")]
    Timeout(String),
    /// Запись в production-профиль (или выгрузка его данных) без разрешения
    /// человека — отказ барьера до подключения.
    #[error("{0}")]
    ProdGuard(String),
    /// Подключение к Postgres не состоялось. Текст тот же, что у [`AppError::Pg`];
    /// отдельный вариант нужен, чтобы по коду отличать отказ входа (пароль,
    /// таймаут, нет базы) от ошибки уже открытой сессии.
    #[error("{}", format_pg_error(.0))]
    Connect(tokio_postgres::Error),
}

impl AppError {
    /// Машиночитаемый код для фронта и CLI — вместо разбора текста ошибки.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Msg(_) => "app",
            AppError::NotFound(_) => "not_found",
            AppError::Usage(_) => "usage",
            AppError::Auth(_) => "auth",
            AppError::Config(_) => "config",
            AppError::Network(_) => "network",
            AppError::Timeout(_) => "timeout",
            AppError::ProdGuard(_) => "prod_guard",
            AppError::Connect(e) => connect_kind(e),
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
            AppError::Pg(e) | AppError::Connect(e) => e.as_db_error().map(|db| db.code().code()),
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

/// Класс отказа подключения. `Kind` у tokio-postgres приватный, а его Display
/// печатает только название вида без причины («error connecting to server»),
/// поэтому смотрим на SQLSTATE, io-причину и текст причины.
fn connect_kind(e: &tokio_postgres::Error) -> &'static str {
    use std::error::Error as _;
    if let Some(db) = e.as_db_error() {
        let code = db.code();
        return if code.code().starts_with("28") {
            // 28000 / 28P01: роль или пароль не подошли (pg_hba, неверный пароль)
            "auth"
        } else if *code == SqlState::INVALID_CATALOG_NAME {
            "not_found"
        } else {
            "db"
        };
    }
    let cause = e.source();
    if let Some(io) = cause.and_then(|s| s.downcast_ref::<std::io::Error>()) {
        return if io.kind() == std::io::ErrorKind::TimedOut {
            "timeout"
        } else {
            "network"
        };
    }
    // Сервер просит пароль, а его нет — tokio-postgres зовёт это «invalid
    // configuration: password missing».
    if cause.is_some_and(|s| s.to_string() == "password missing") {
        return "auth";
    }
    match e.to_string().as_str() {
        "authentication error" => "auth",
        "invalid configuration" => "config",
        "timeout waiting for server" => "timeout",
        _ => "network",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Отказ в соединении — сеть, а не потеря сессии: сессии ещё не было.
    /// Текст остаётся тем же, что у `Pg`, — его видит GUI.
    #[tokio::test]
    async fn refused_connect_is_network_with_the_pg_text() {
        // слушатель закрывается в конце выражения — порт свободен и молчит
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut cfg = tokio_postgres::Config::new();
        cfg.host("127.0.0.1")
            .port(port)
            .user("nobody")
            .connect_timeout(std::time::Duration::from_secs(2));
        let inner = match cfg.connect(tokio_postgres::NoTls).await {
            Ok(_) => panic!("на порту {port} неожиданно ответил Postgres"),
            Err(e) => e,
        };
        let text = format_pg_error(&inner);
        let e = AppError::Connect(inner);
        assert_eq!(e.code(), "network");
        assert_eq!(e.to_string(), text);
    }

    #[test]
    fn new_variants_keep_the_message_and_name_their_class() {
        for (e, code) in [
            (
                AppError::NotFound("профиль 'x' не найден".into()),
                "not_found",
            ),
            (AppError::Usage("нет SQL".into()), "usage"),
            (AppError::Auth("vault заблокирован".into()), "auth"),
            (AppError::Config("vault не создан".into()), "config"),
            (AppError::Network("ssh tunnel exited".into()), "network"),
            (AppError::Timeout("ssh tunnel: timed out".into()), "timeout"),
            (
                AppError::ProdGuard("запись заблокирована".into()),
                "prod_guard",
            ),
        ] {
            assert_eq!(e.code(), code);
            let json = serde_json::to_value(&e).unwrap();
            assert_eq!(json["code"], code);
            assert_eq!(json["message"], e.to_string());
        }
    }
}
