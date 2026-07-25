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

/// Разрешение на запись в профили с меткой `production` без интерактивного
/// подтверждения: `1` — любой прод-профиль, иначе список имён/id через запятую
/// (см. `session::prod`). Для MCP это единственный способ разрешить запись.
pub const ALLOW_PROD_WRITE: &str = "SQL_KAI_ALLOW_PROD_WRITE";
