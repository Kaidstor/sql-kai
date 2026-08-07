//! Авто-маскирование чувствительных колонок в результатах запроса: вывод CLI
//! оседает в логах агентских сессий, поэтому креды (password/secret/token/…)
//! по умолчанию заменяются на [redacted]. Отключение — флаг --no-redact.
//!
//! ВАЖНО: это guardrail от случайной утечки по имени колонки, а не граница
//! безопасности. `SELECT password AS p` или `--no-redact` обходят маскирование
//! — оператор/агент, которому нельзя видеть секрет, не должен иметь доступа к
//! самой БД. Матчинг намеренно склоняется к over-masking (лучше скрыть не-секрет).

use sql_kai_lib::db::ExecResult;

pub const REDACTED: &str = "[redacted]";

/// Имя колонки похоже на креды? Сравнение без учёта регистра:
/// вхождение password/passwd/pwd/secret/passphrase/credential/privatekey/
/// authorization, суффикс token/_key/apikey/_salt/_jwt/_hash, либо точное salt/jwt.
pub fn is_sensitive_column(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const CONTAINS: &[&str] = &[
        "password",
        "passwd",
        "pwd",
        "secret",
        "passphrase",
        "credential",
        "privatekey",
        "authorization",
    ];
    const SUFFIXES: &[&str] = &["token", "_key", "apikey", "_salt", "_jwt", "_hash"];
    const EXACT: &[&str] = &["salt", "jwt"];
    CONTAINS.iter().any(|p| n.contains(p))
        || SUFFIXES.iter().any(|s| n.ends_with(s))
        || EXACT.contains(&n.as_str())
}

/// Маскирует значения чувствительных колонок во всех результатах (NULL
/// остаётся NULL). Возвращает имена колонок, где что-то реально закрыто, —
/// для предупреждения в stderr.
pub fn redact_exec(exec: &mut ExecResult) -> Vec<String> {
    let mut masked: Vec<String> = Vec::new();
    for r in &mut exec.results {
        let sensitive: Vec<usize> = r
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| is_sensitive_column(c))
            .map(|(i, _)| i)
            .collect();
        if sensitive.is_empty() {
            continue;
        }
        let mut touched = vec![false; sensitive.len()];
        for row in &mut r.rows {
            for (k, &i) in sensitive.iter().enumerate() {
                if let Some(cell) = row.get_mut(i) {
                    if cell.is_some() {
                        *cell = Some(REDACTED.to_string());
                        touched[k] = true;
                    }
                }
            }
        }
        for (k, &i) in sensitive.iter().enumerate() {
            if touched[k] && !masked.contains(&r.columns[i]) {
                masked.push(r.columns[i].clone());
            }
        }
    }
    masked
}

#[cfg(test)]
mod tests {
    use super::*;
    use sql_kai_lib::db::{ExecResult, StatementResult};

    #[test]
    fn detects_sensitive_names() {
        for name in [
            "password",
            "password_hash",
            "PASSWORD",
            "passwd",
            "user_secret",
            "secret_2fa",
            "access_token",
            "token",
            "refresh_token",
            "authtoken",
            "api_key",
            "private_key",
            "privatekey",
            "apikey",
            "passphrase",
            "credentials",
            "salt",
            "pwd_salt",
            "user_pwd",
            "authorization",
            "token_hash",
            "jwt",
        ] {
            assert!(is_sensitive_column(name), "{name} должен маскироваться");
        }
        for name in [
            "id",
            "email",
            "name",
            "monkey",
            "token_count",
            "keyboard",
            "primary",
            "author",
        ] {
            assert!(!is_sensitive_column(name), "{name} не должен маскироваться");
        }
    }

    #[test]
    fn masks_values_and_keeps_nulls() {
        let mut exec = ExecResult {
            results: vec![StatementResult {
                columns: vec!["id".into(), "password_hash".into()],
                rows: vec![
                    vec![Some("1".into()), Some("$argon2id$…".into())],
                    vec![Some("2".into()), None],
                ],
                rows_affected: None,
                truncated: false,
            }],
            duration_ms: 0,
        };
        let masked = redact_exec(&mut exec);
        assert_eq!(masked, vec!["password_hash".to_string()]);
        assert_eq!(exec.results[0].rows[0][0].as_deref(), Some("1"));
        assert_eq!(exec.results[0].rows[0][1].as_deref(), Some(REDACTED));
        assert_eq!(exec.results[0].rows[1][1], None);
    }

    #[test]
    fn all_null_column_is_not_reported() {
        let mut exec = ExecResult {
            results: vec![StatementResult {
                columns: vec!["secret".into()],
                rows: vec![vec![None]],
                rows_affected: None,
                truncated: false,
            }],
            duration_ms: 0,
        };
        assert!(redact_exec(&mut exec).is_empty());
    }
}
