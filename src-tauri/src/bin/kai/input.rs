//! Сбор SQL-текста запроса для `kai q` / `kai exec`.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use sql_kai_lib::error::AppError;

/// SQL из -c/-f, иначе со stdin (если это пайп).
pub fn collect_sql(commands: &[String], files: &[PathBuf]) -> Result<String, AppError> {
    let mut parts: Vec<String> = Vec::new();
    for c in commands {
        parts.push(format!("{};", c.trim_end().trim_end_matches(';')));
    }
    for f in files {
        let text = std::fs::read_to_string(f)
            .map_err(|e| AppError::Msg(format!("чтение {}: {e}", f.display())))?;
        parts.push(text);
    }
    if parts.is_empty() && !std::io::stdin().is_terminal() {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        if !s.trim().is_empty() {
            parts.push(s);
        }
    }
    let sql = parts.join("\n");
    if sql.trim().is_empty() {
        return Err(AppError::Msg(
            "нет SQL: передай -c \"...\", -f file.sql или подай на stdin".into(),
        ));
    }
    Ok(sql)
}
