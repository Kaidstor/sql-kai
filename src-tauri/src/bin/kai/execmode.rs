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

use crate::remote::{remote_command, ssh_base, CONTAINER_DETECT};
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

/// Хвост поверх [`CONTAINER_DETECT`]: заливает переданный SQL (base64 в env) во
/// временный файл и прогоняет его через `docker exec psql` внутри контейнера.
const EXEC_TAIL: &str = r#"if [ -n "${KAI_VERBOSE:-}" ]; then echo "[container=$C user=$U db=$DB]" >&2; fi
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
    let script = format!("{CONTAINER_DETECT}{EXEC_TAIL}");
    let remote = remote_command(&script, &env);

    if a.dry_run {
        println!("ssh -T -o BatchMode=yes -o ConnectTimeout=15 {} \\\n  {remote}", a.alias);
        println!("\n# скрипт на хосте:\n{script}");
        return Ok(ExitCode::SUCCESS);
    }

    let status = ssh_base(&a.alias)
        .arg(remote)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| AppError::Msg(format!("ssh: {e}")))?;
    Ok(ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8))
}
