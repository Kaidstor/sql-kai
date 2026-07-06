//! Общая ssh-обвязка для discover/exec: скрипт уезжает на хост одним
//! аргументом (base64, чтобы не воевать с кавычками), env — префиксом.

use std::process::Command;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

/// POSIX-shell single-quote.
pub fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `VAR='...' VAR2='...' bash -c "$(printf %s '<b64>' | base64 -d)"`
pub fn remote_command(script: &str, env: &[(&str, String)]) -> String {
    let prefix: String = env
        .iter()
        .map(|(k, v)| format!("{k}={} ", shq(v)))
        .collect();
    format!(
        "{prefix}bash -c \"$(printf %s {} | base64 -d)\"",
        shq(&B64.encode(script))
    )
}

/// Базовая ssh-команда: без TTY, только неинтерактивная аутентификация
/// (ключи/agent из ~/.ssh/config), быстрый фейл вместо зависшего промпта.
pub fn ssh_base(alias: &str) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=15"])
        .arg(alias);
    cmd
}
