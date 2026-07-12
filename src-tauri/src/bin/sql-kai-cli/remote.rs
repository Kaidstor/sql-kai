//! Общая ssh-обвязка для discover/exec: скрипт уезжает на хост через stdin
//! (`bash -s`), argv содержит только alias — ни SQL, ни env-значения не
//! светятся в `ps` ни локально, ни на удалённом хосте.

use std::io::Write;
use std::process::{Command, ExitStatus, Output, Stdio};

/// Общий пролог remote-скриптов discover/exec: находит postgres-контейнер и
/// определяет POSTGRES_USER/DB (с фолбэком на живой psql-запрос). Каждая команда
/// дописывает свой хвост — единственная копия этого блока (раньше он дублировался
/// байт-в-байт в discover.rs и execmode.rs).
///
/// Непустой `KAI_CONTAINER` (экспортируется из `--container`) выбирает контейнер
/// явно — на хосте их может быть несколько (db_admin + db_app и т.п.); без него
/// берётся первый кандидат, а при нескольких печатается предупреждение со списком.
pub const CONTAINER_DETECT: &str = r#"set -u
D=docker
docker ps >/dev/null 2>&1 || D="sudo docker"
if [ -n "${KAI_CONTAINER:-}" ]; then
  C=$($D ps --format '{{.Names}}' 2>/dev/null | grep -xF -- "$KAI_CONTAINER")
  [ -n "$C" ] || { echo "sql-kai: контейнер '$KAI_CONTAINER' не найден среди запущенных (docker ps)" >&2; exit 3; }
else
  CANDS=$($D ps --format '{{.Names}}' 2>/dev/null | grep -iE '(^|[-_])db([-_]|$)|postgres')
  C=$(printf '%s\n' "$CANDS" | head -1)
  [ -n "$C" ] || { echo 'sql-kai: postgres-контейнер не найден в docker ps' >&2; exit 3; }
  if [ "$(printf '%s\n' "$CANDS" | grep -c .)" -gt 1 ]; then
    echo "sql-kai: на хосте несколько postgres-контейнеров ($(printf '%s ' $CANDS)) — выбран '$C', другой задаётся через --container" >&2
  fi
fi
U=$($D exec "$C" printenv POSTGRES_USER 2>/dev/null)
[ -n "$U" ] || U=postgres
DB=$($D exec "$C" printenv POSTGRES_DB 2>/dev/null)
[ -n "$DB" ] || DB=$($D exec "$C" psql -U "$U" -tAc "SELECT datname FROM pg_database WHERE NOT datistemplate ORDER BY datname = 'postgres', datname LIMIT 1" 2>/dev/null | tr -d '[:space:]')
[ -n "$DB" ] || { echo 'sql-kai: не удалось определить имя базы' >&2; exit 5; }
"#;

/// POSIX-shell single-quote.
pub fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Полный скрипт для `bash -s`: экспорты env + тело. Уходит через stdin,
/// поэтому значения (SQL, опции psql) не попадают в argv.
pub fn stdin_payload(script: &str, env: &[(&str, String)]) -> String {
    let mut full = String::new();
    for (k, v) in env {
        full.push_str(&format!("export {k}={}\n", shq(v)));
    }
    full.push_str(script);
    full
}

/// Базовая ssh-команда: без TTY, только неинтерактивная аутентификация
/// (ключи/agent из ~/.ssh/config), быстрый фейл вместо зависшего промпта.
/// `--` отсекает alias вида `-oProxyCommand=…`; мастер-пароль vault детям
/// не наследуется.
pub fn ssh_base(alias: &str) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=15"])
        .arg("--")
        .arg(alias);
    cmd.env_remove("KAI_VAULT_PASSWORD");
    cmd
}

/// Выполняет payload на хосте (`bash -s` + stdin), stdout/stderr — в наш tty.
pub fn run_via_stdin(alias: &str, payload: &str) -> std::io::Result<ExitStatus> {
    let mut child = ssh_base(alias)
        .args(["bash", "-s"])
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(payload.as_bytes())?;
    child.wait()
}

/// То же, но с захватом stdout/stderr (для discover).
pub fn output_via_stdin(alias: &str, payload: &str) -> std::io::Result<Output> {
    let mut child = ssh_base(alias)
        .args(["bash", "-s"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(payload.as_bytes())?;
    child.wait_with_output()
}
