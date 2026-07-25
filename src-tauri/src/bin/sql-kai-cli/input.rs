//! Ввод от пользователя: сбор SQL-текста для `sql-kai q` / `sql-kai exec` и
//! подтверждение опасных шагов.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use sql_kai_lib::error::AppError;

/// Есть ли на том конце человек. Отдельно от [`confirm`] для команд, которым
/// без TTY нужно не упасть, а сделать что-то другое (feedback печатает ссылку).
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal()
}

/// Вопрос «да/нет». Единственная реализация на весь CLI: копий было пять, и они
/// разъехались по набору принимаемых ответов — «да» в одной команде значило
/// согласие, а в соседней молча означало отмену.
///
/// `skip_flag` — чем шаг пропускается неинтерактивно; попадает в текст ошибки,
/// когда TTY нет и спросить некого.
pub fn confirm(question: &str, skip_flag: &str) -> Result<bool, AppError> {
    if !is_interactive() {
        return Err(AppError::Msg(format!(
            "нет TTY для подтверждения — добавь {skip_flag}"
        )));
    }
    eprint!("{question} [y/N] ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_lowercase().as_str(),
        "y" | "yes" | "д" | "да"
    ))
}

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
