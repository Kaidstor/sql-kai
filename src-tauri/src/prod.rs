//! Общая политика production-профилей: разбор allowlist-значений
//! `SQL_KAI_ALLOW_PROD_WRITE` / `SQL_KAI_ALLOW_PROD_DUMP`. Security-sensitive
//! разбор живёт в одном месте — CLI (`session::prod`) и сервер сессий
//! (`broker::server`) обязаны трактовать значение одинаково, иначе разрешение,
//! выданное для одной стороны, молча оказывается шире на другой.

/// Разрешение на запись в профили с меткой `production` без интерактивного
/// подтверждения: `1` — любой прод-профиль, иначе список имён/id через запятую
/// (см. `session::prod` в CLI). Для MCP это единственный способ разрешить
/// запись.
pub const ALLOW_PROD_WRITE: &str = "SQL_KAI_ALLOW_PROD_WRITE";

/// То же для выгрузки боевых данных на эту машину (`fork --data` с
/// production-профиля): значения разбираются так же, как у [`ALLOW_PROD_WRITE`].
pub const ALLOW_PROD_DUMP: &str = "SQL_KAI_ALLOW_PROD_DUMP";

/// Значение allowlist-переменной: `1`/`true`/`yes`/`on`/`all` (любой
/// prod-профиль) или список имён/id профилей через запятую — разрешение
/// точечное, чтобы «открыть» один сервис агенту, не открывая остальные.
pub fn allowlist_matches(raw: &str, name: &str, id: &str) -> bool {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .any(|item| {
            matches!(
                item.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "all"
            ) || item.eq_ignore_ascii_case(name)
                || item == id
        })
}

/// Env-переменная `var` в окружении *этого* процесса разрешает профиль
/// (name, id)? Пустая строка — «не задана».
pub fn env_allows(var: &str, name: &str, id: &str) -> bool {
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .is_some_and(|raw| allowlist_matches(&raw, name, id))
}

#[cfg(test)]
mod tests {
    use super::allowlist_matches;

    #[test]
    fn wildcard_values_allow_any_profile() {
        for raw in ["1", "true", "YES", "on", "all"] {
            assert!(allowlist_matches(raw, "domainator", "id-1"));
        }
    }

    #[test]
    fn allowlist_is_per_profile() {
        assert!(allowlist_matches("vuln, domainator", "domainator", "id-1"));
        assert!(allowlist_matches("DOMAINATOR", "domainator", "id-1"));
        assert!(allowlist_matches("id-1", "domainator", "id-1"));
        assert!(!allowlist_matches("vuln", "domainator", "id-1"));
        assert!(!allowlist_matches("", "domainator", "id-1"));
        // id — точное совпадение: регистр в uuid значим
        assert!(!allowlist_matches("ID-1", "domainator", "id-1"));
    }
}
