//! Реестр публичных env-переменных: единый неймспейс `SQL_KAI_*`, читаемый и
//! GUI, и CLI (`crate::envvar` в бинаре — реэкспорт этого модуля). Имя каждой
//! переменной объявлено здесь ровно один раз: строковый литерал на месте
//! использования выпадает из реестра и расходится с документацией молча.
//!
//! Внутренние переменные ssh-payload (`KAI_SQL_B64`, `KAI_CONTAINER` и т.п.)
//! сюда не относятся: их задаёт сам payload на удалённом хосте, наружу как
//! настройка они не торчат.

/// Значение переменной; пустая строка трактуется как «не задана».
pub fn value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Каталог конфигурации вместо `~/Library/Application Support/sql-kai`
/// (изолированные прогоны, тесты).
pub const CONFIG_DIR: &str = "SQL_KAI_CONFIG_DIR";

/// Мастер-пароль vault для неинтерактивного разлока.
pub const VAULT_PASSWORD: &str = "SQL_KAI_VAULT_PASSWORD";

/// Passphrase ssh-ключа: ssh читает её через SSH_ASKPASS-хелпер, поэтому
/// секрет остаётся в окружении и не попадает в argv.
pub const SSH_PASSPHRASE: &str = "SQL_KAI_SSH_PASSPHRASE";

/// Путь к бинарю `sec`; без неё — `sec` из PATH.
pub const SEC_BIN: &str = "SQL_KAI_SEC_BIN";

/// TTL персистентного ssh-мастера (ControlMaster), секунды.
pub const SSH_MUX_TTL: &str = "SQL_KAI_SSH_MUX_TTL";

/// Разрешение на запись в профили с меткой `production` без интерактивного
/// подтверждения: `1` — любой прод-профиль, иначе список имён/id через запятую
/// (разбор — [`crate::prod::allowlist_matches`]). Для MCP это единственный
/// способ разрешить запись.
pub const ALLOW_PROD_WRITE: &str = "SQL_KAI_ALLOW_PROD_WRITE";

/// То же для выгрузки боевых данных на эту машину (`fork --data` с
/// production-профиля): значения разбираются так же, как у [`ALLOW_PROD_WRITE`].
pub const ALLOW_PROD_DUMP: &str = "SQL_KAI_ALLOW_PROD_DUMP";
