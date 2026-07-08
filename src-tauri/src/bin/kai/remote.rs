//! Общая ssh-обвязка для discover/exec: скрипт уезжает на хост одним
//! аргументом (base64, чтобы не воевать с кавычками), env — префиксом.

use std::process::Command;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

/// Общий пролог remote-скриптов discover/exec: находит postgres-контейнер и
/// определяет POSTGRES_USER/DB (с фолбэком на живой psql-запрос). Каждая команда
/// дописывает свой хвост — единственная копия этого блока (раньше он дублировался
/// байт-в-байт в discover.rs и execmode.rs).
pub const CONTAINER_DETECT: &str = r#"set -u
D=docker
docker ps >/dev/null 2>&1 || D="sudo docker"
C=$($D ps --format '{{.Names}}' 2>/dev/null | grep -iE '(^|[-_])db([-_]|$)|postgres' | head -1)
[ -n "$C" ] || { echo 'kai: postgres-контейнер не найден в docker ps' >&2; exit 3; }
U=$($D exec "$C" printenv POSTGRES_USER 2>/dev/null)
[ -n "$U" ] || U=postgres
DB=$($D exec "$C" printenv POSTGRES_DB 2>/dev/null)
[ -n "$DB" ] || DB=$($D exec "$C" psql -U "$U" -tAc "SELECT datname FROM pg_database WHERE datname NOT IN ('postgres','template0','template1') ORDER BY datname LIMIT 1" 2>/dev/null | tr -d '[:space:]')
[ -n "$DB" ] || { echo 'kai: не удалось определить имя базы' >&2; exit 5; }
"#;

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
