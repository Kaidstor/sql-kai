//! `kai exec <ssh-alias>` — fallback без профиля и vault (наследник prod-db):
//! ssh на хост -> найти postgres-контейнер -> docker exec psql. Работает даже
//! когда порт наружу не открыт и пароль неизвестен (psql внутри контейнера
//! ходит по trust/peer). Вывод — сырой текст psql.

use std::path::PathBuf;
use std::process::{ExitCode, Stdio};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use clap::Args;
use sql_tauri_lib::error::AppError;

use crate::remote::{remote_command, ssh_base};
use crate::session;

#[derive(Args)]
pub struct ExecArgs {
    /// SSH-алиас хоста (из ~/.ssh/config*)
    alias: String,
    /// SQL-команда (можно повторять)
    #[arg(short = 'c', long = "command", value_name = "SQL")]
    commands: Vec<String>,
    /// Файл с SQL (можно повторять)
    #[arg(short = 'f', long = "file", value_name = "FILE")]
    files: Vec<PathBuf>,
    /// tuples-only без рамок (psql -tA)
    #[arg(short = 't', long)]
    tuples: bool,
    /// Вывод в CSV
    #[arg(long)]
    csv: bool,
    /// Разрешить запись (по умолчанию сессия read-only)
    #[arg(long)]
    write: bool,
    /// Показать резолв контейнера/юзера/базы
    #[arg(short, long)]
    verbose: bool,
    /// Показать команду, не заходя на хост
    #[arg(long)]
    dry_run: bool,
}

const REMOTE_SCRIPT: &str = r#"set -u
D=docker
docker ps >/dev/null 2>&1 || D="sudo docker"
C=$($D ps --format '{{.Names}}' 2>/dev/null | grep -iE '(^|[-_])db([-_]|$)|postgres' | head -1)
[ -n "$C" ] || { echo 'kai: postgres-контейнер не найден в docker ps' >&2; exit 3; }
U=$($D exec "$C" printenv POSTGRES_USER 2>/dev/null)
[ -n "$U" ] || U=postgres
DB=$($D exec "$C" printenv POSTGRES_DB 2>/dev/null)
[ -n "$DB" ] || DB=$($D exec "$C" psql -U "$U" -tAc "SELECT datname FROM pg_database WHERE datname NOT IN ('postgres','template0','template1') ORDER BY datname LIMIT 1" 2>/dev/null | tr -d '[:space:]')
[ -n "$DB" ] || { echo 'kai: не удалось определить имя базы' >&2; exit 5; }
if [ -n "${KAI_VERBOSE:-}" ]; then echo "[container=$C user=$U db=$DB]" >&2; fi
SQLF=$(mktemp)
trap 'rm -f "$SQLF"' EXIT
printf '%s' "${KAI_SQL_B64:-}" | base64 -d > "$SQLF"
$D exec -i "$C" psql -U "$U" -d "$DB" -v ON_ERROR_STOP=1 ${KAI_PSQL_OPTS:-} -f - < "$SQLF"
"#;

pub fn run(a: ExecArgs) -> Result<ExitCode, AppError> {
    let mut sql = session::collect_sql(&a.commands, &a.files)?;
    if !a.write {
        sql = format!("SET default_transaction_read_only = on;\n{sql}");
    }
    let mut psql_opts: Vec<&str> = Vec::new();
    if a.tuples {
        psql_opts.extend(["-t", "-A"]);
    }
    if a.csv {
        psql_opts.push("--csv");
    }
    let env = [
        ("KAI_SQL_B64", B64.encode(&sql)),
        ("KAI_PSQL_OPTS", psql_opts.join(" ")),
        ("KAI_VERBOSE", if a.verbose { "1" } else { "" }.to_string()),
    ];
    let remote = remote_command(REMOTE_SCRIPT, &env);

    if a.dry_run {
        println!("ssh -T -o BatchMode=yes -o ConnectTimeout=15 {} \\\n  {remote}", a.alias);
        println!("\n# скрипт на хосте:\n{REMOTE_SCRIPT}");
        return Ok(ExitCode::SUCCESS);
    }

    let status = ssh_base(&a.alias)
        .arg(remote)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| AppError::Msg(format!("ssh: {e}")))?;
    Ok(ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8))
}
