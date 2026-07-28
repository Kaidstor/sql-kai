//! Публичные env-переменные CLI. Единый неймспейс — префикс `SQL_KAI_*`
//! (`SQL_KAI_VAULT_PASSWORD`, `SQL_KAI_CONFIG_DIR`, `SQL_KAI_SSH_PASSPHRASE`,
//! `SQL_KAI_SEC_BIN`, `SQL_KAI_SSH_MUX_TTL`, `SQL_KAI_ALLOW_PROD_WRITE`).
//!
//! Внутренние переменные ssh-payload (`KAI_SQL_B64`, `KAI_CONTAINER` и т.п.)
//! сюда не относятся: их задаёт сам payload на удалённом хосте, наружу как
//! настройка они не торчат.

/// Значение переменной; пустая строка трактуется как «не задана».
pub fn value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Путь к бинарю `sec`; без неё — `sec` из PATH.
pub const SEC_BIN: &str = "SQL_KAI_SEC_BIN";

/// TTL персистентного ssh-мастера (ControlMaster), секунды.
pub const SSH_MUX_TTL: &str = "SQL_KAI_SSH_MUX_TTL";

// Имена и разбор прод-allowlist-переменных общие с сервером сессий — живут в
// библиотеке (sql_kai_lib::prod), здесь только реэкспорт в общий неймспейс.
pub use sql_kai_lib::prod::{ALLOW_PROD_DUMP, ALLOW_PROD_WRITE};
