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
/// `KAI_STRICT` превращает эту неоднозначность в отказ (код 4) — для команд, где
/// «не тот кластер» дороже отказа: fork скопировал бы чужую базу под тем же
/// именем, и миграцию проверили бы не на той схеме.
pub const CONTAINER_FIND: &str = r#"set -u
D=docker
docker ps >/dev/null 2>&1 || D="sudo docker"
# Кандидаты: сначала запущенные, затем — если KAI_ANY_STATE задан — остановленные.
# `docker logs` читается и у остановленного контейнера, а вот `docker exec` нет,
# поэтому расширение состояний включает только logs.
kai_names() {
  $D ps --format '{{.Names}}' 2>/dev/null
  [ -n "${KAI_ANY_STATE:-}" ] && $D ps -a --format '{{.Names}}' 2>/dev/null
  return 0
}
NAMES=$(kai_names | awk 'NF && !seen[$0]++')
if [ -n "${KAI_CONTAINER:-}" ]; then
  C=$(printf '%s\n' "$NAMES" | grep -xF -- "$KAI_CONTAINER")
  [ -n "$C" ] || { echo "sql-kai: контейнер '$KAI_CONTAINER' не найден (docker ps${KAI_ANY_STATE:+ -a})" >&2; exit 3; }
else
  CANDS=""
  if [ -n "${KAI_PORT:-}" ]; then
    # Опубликованный порт профиля — признак, не зависящий от имени контейнера:
    # у форков оно своё (sql-kai-fork-*) и под именную эвристику не подходит.
    # Берём из inspect, а не из `docker port`: у остановленного контейнера порты
    # не опубликованы, а привязка в конфиге осталась — и журнал у него читается.
    for n in $NAMES; do
      HP=$($D inspect --format '{{range $p, $c := .HostConfig.PortBindings}}{{range $c}}{{.HostPort}} {{end}}{{end}}' "$n" 2>/dev/null)
      case " $HP " in *" ${KAI_PORT} "*) CANDS="$CANDS$n
";; esac
    done
    # Порт знаем — значит он и решает. Откат на поиск по имени тут недопустим:
    # на машине разработчика он находит десяток чужих postgres-контейнеров и
    # молча выдаёт журнал не той базы.
    [ -n "$CANDS" ] || { echo "sql-kai: нет контейнера, публикующего порт $KAI_PORT — задай его явно через --container (список: docker ps -a)" >&2; exit 3; }
  else
    CANDS=$(printf '%s\n' "$NAMES" | grep -iE '(^|[-_])db([-_]|$)|postgres')
  fi
  C=$(printf '%s\n' "$CANDS" | awk 'NF' | head -1)
  [ -n "$C" ] || { echo 'sql-kai: postgres-контейнер не найден в docker ps' >&2; exit 3; }
  if [ "$(printf '%s\n' "$CANDS" | grep -c .)" -gt 1 ]; then
    if [ -n "${KAI_STRICT:-}" ]; then
      echo "sql-kai: несколько подходящих контейнеров ($(printf '%s ' $CANDS)) — выбери нужный через --container" >&2
      exit 4
    fi
    echo "sql-kai: несколько подходящих контейнеров ($(printf '%s ' $CANDS)) — выбран '$C', другой задаётся через --container" >&2
  fi
fi
"#;

/// Довесок к [`CONTAINER_FIND`] для тех, кому нужен не только контейнер, но и
/// роль с базой внутри него. Требует живого `docker exec` (и отвечающего
/// postgres, когда POSTGRES_DB не задан), поэтому `logs` его НЕ берёт: журнал
/// нужен как раз тогда, когда psql внутри уже не отвечает.
pub const CONTAINER_DB_ENV: &str = r#"U=$($D exec "$C" printenv POSTGRES_USER 2>/dev/null)
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
    sql_kai_lib::vault::scrub_master_password_env(&mut cmd);
    cmd
}

/// Выполняет payload на хосте (`bash -s` + stdin), stdout/stderr — в наш tty.
///
/// Pty здесь нет (`-T`), поэтому ни Ctrl+C, ни разрыв соединения до хоста не
/// доходят: наш ssh умирает, а долгоживущий процесс на той стороне узнаёт о
/// закрытом пайпе только при следующей записи (для `docker logs --follow` на
/// молчащей базе — никогда). Скрипты с бесконечными командами должны сами
/// сторожить смерть родителя (см. `LOGS_TAIL` в cmd/logs.rs).
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
