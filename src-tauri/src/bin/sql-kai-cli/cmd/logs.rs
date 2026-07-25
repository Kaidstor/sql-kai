//! `sql-kai logs <alias>` — журнал postgres-сервера профиля: ssh на хост из
//! ssh-конфига профиля -> `docker logs` его контейнера. Профили от `sql-kai
//! discover` уже знают ssh-хост, так что ответ на «почему запрос упал / что
//! происходит на сервере» достаётся без похода на хост руками — и, в отличие от
//! запросов к `pg_stat_*`, работает даже когда postgres соединения уже не
//! принимает (упал, перезапускается, забил диск).

use std::io::Write;
use std::process::{Command, ExitCode, ExitStatus, Stdio};

use clap::Args;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::Profile;

use crate::remote::{self, CONTAINER_DETECT};
use crate::session;

#[derive(Args)]
pub struct LogsArgs {
    /// Профиль: имя, id или группа
    alias: String,
    /// Сколько последних строк: число или all
    #[arg(short = 'n', long, value_name = "N", default_value = "500")]
    tail: String,
    /// Стримить новые строки до Ctrl+C
    #[arg(short = 'f', long)]
    follow: bool,
    /// Только свежее указанного: длительность (10m, 1h30m) или RFC3339
    #[arg(long, value_name = "WHEN")]
    since: Option<String>,
    /// Только старше указанного: длительность (10m, 1h30m) или RFC3339
    #[arg(long, value_name = "WHEN")]
    until: Option<String>,
    /// Добавить метку времени docker к каждой строке
    #[arg(long)]
    timestamps: bool,
    /// Docker-контейнер postgres (когда на хосте их несколько)
    #[arg(long, value_name = "NAME")]
    container: Option<String>,
    /// Показать резолв контейнера
    #[arg(short, long)]
    verbose: bool,
    /// Показать команду, не заходя на хост
    #[arg(long)]
    dry_run: bool,
}

/// Хвост поверх [`CONTAINER_DETECT`]: собирает опции `docker logs` в позиционные
/// параметры — каждое значение отдельным аргументом в кавычках, поэтому пробелы
/// и спецсимволы во времени/числе строк не расщепляются на лишние флаги (в
/// отличие от `${KAI_PSQL_OPTS}` в exec, где опции задаёт не пользователь).
/// Первый `set --` всегда с `--tail`: пустой `"$@"` под `set -u` роняет bash 3.2
/// (локальный docker на macOS).
///
/// Postgres пишет журнал в stderr, поэтому сливаем его в stdout: иначе привычное
/// `sql-kai logs prod | grep ERROR` не увидело бы ни строчки. `exec` вместо
/// вызова — чтобы при `--follow` поток шёл без лишнего bash в середине.
const LOGS_TAIL: &str = r#"if [ -n "${KAI_VERBOSE:-}" ]; then echo "[container=$C]" >&2; fi
set -- --tail "${KAI_TAIL:-500}"
[ -n "${KAI_SINCE:-}" ] && set -- "$@" --since "$KAI_SINCE"
[ -n "${KAI_UNTIL:-}" ] && set -- "$@" --until "$KAI_UNTIL"
[ -n "${KAI_TIMESTAMPS:-}" ] && set -- "$@" --timestamps
[ -n "${KAI_FOLLOW:-}" ] && set -- "$@" --follow
exec $D logs "$@" "$C" 2>&1
"#;

/// Где крутится docker с базой профиля.
enum Target {
    /// ssh-алиас из профиля (обычный случай — профиль после `sql-kai discover`).
    Ssh(String),
    /// Профиль без ssh, смотрящий в петлю: docker на этой же машине.
    Local,
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]")
}

/// Профиль без ssh и не в петле — это либо managed-база, либо postgres не в
/// docker: `docker logs` там взять неоткуда, поэтому честно говорим об этом.
fn resolve_target(p: &Profile) -> Result<Target, AppError> {
    if let Some(ssh) = &p.ssh {
        return Ok(Target::Ssh(ssh.host.clone()));
    }
    if is_loopback(&p.host) {
        return Ok(Target::Local);
    }
    Err(AppError::Msg(format!(
        "профиль '{}' ходит в {}:{} напрямую, без ssh — logs умеет только postgres в docker \
         (профиль из `sql-kai discover` или локальный контейнер). Логи managed-базы смотри \
         в панели провайдера",
        p.name, p.host, p.port
    )))
}

fn validate_tail(v: &str) -> Result<(), AppError> {
    if v == "all" || (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())) {
        return Ok(());
    }
    Err(AppError::Msg(format!(
        "--tail: ожидается число строк или 'all', получено '{v}'"
    )))
}

/// `docker logs` отличает опции от значений по ведущему дефису, поэтому
/// `--since -f` отдавать ему нельзя: подстановка пришла бы флагом.
fn validate_when(flag: &str, v: &str) -> Result<(), AppError> {
    if v.is_empty() {
        return Err(AppError::Msg(format!("{flag}: пустое значение")));
    }
    if v.starts_with('-') {
        return Err(AppError::Msg(format!(
            "{flag}: значение не может начинаться с '-' (получено '{v}'); \
             длительность задаётся как 10m/1h30m, момент — RFC3339"
        )));
    }
    Ok(())
}

fn flag(on: bool) -> String {
    if on { "1" } else { "" }.to_string()
}

/// Локальный аналог [`remote::run_via_stdin`]: тот же скрипт, но без ssh.
/// Мастер-пароль vault ребёнку не наследуем — как и при спавне ssh.
fn run_local(payload: &str) -> std::io::Result<ExitStatus> {
    let mut cmd = Command::new("bash");
    cmd.args(["-s"]).stdin(Stdio::piped());
    sql_kai_lib::vault::scrub_master_password_env(&mut cmd);
    let mut child = cmd.spawn()?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(payload.as_bytes())?;
    child.wait()
}

/// Коды из [`CONTAINER_DETECT`]: 3 — контейнер не опознан, 5 — не определилась
/// база. Для logs пятёрка особенно обидна (журнал нужен как раз когда psql в
/// контейнере уже не отвечает), поэтому даём готовую команду в обход.
fn print_hint(code: i32, a: &LogsArgs, target: &Target) {
    let via = match target {
        Target::Ssh(host) => format!("ssh {host} "),
        Target::Local => String::new(),
    };
    match code {
        3 if a.container.is_some() => {
            eprintln!("hint: список запущенных контейнеров — `{via}docker ps`");
        }
        3 => eprintln!(
            "hint: контейнер не опознан по имени — задай явно: \
             `sql-kai logs {} --container <имя>` (список: `{via}docker ps`). \
             Если postgres на хосте не в docker, logs не поможет: смотри \
             `journalctl -u postgresql` или /var/log/postgresql",
            a.alias
        ),
        5 => eprintln!(
            "hint: контейнер нашёлся, но не определилась база (psql внутри уже не отвечает?) — \
             за журналом сходи напрямую: `{via}docker logs --tail {} <контейнер>`",
            a.tail
        ),
        _ => {}
    }
}

pub fn run(a: LogsArgs) -> Result<ExitCode, AppError> {
    validate_tail(&a.tail)?;
    if let Some(v) = &a.since {
        validate_when("--since", v)?;
    }
    if let Some(v) = &a.until {
        validate_when("--until", v)?;
    }

    let profile = session::resolve_profile(&a.alias)?;
    let target = resolve_target(&profile)?;

    let env = [
        ("KAI_CONTAINER", a.container.clone().unwrap_or_default()),
        ("KAI_TAIL", a.tail.clone()),
        ("KAI_SINCE", a.since.clone().unwrap_or_default()),
        ("KAI_UNTIL", a.until.clone().unwrap_or_default()),
        ("KAI_TIMESTAMPS", flag(a.timestamps)),
        ("KAI_FOLLOW", flag(a.follow)),
        ("KAI_VERBOSE", flag(a.verbose)),
    ];
    let script = format!("{CONTAINER_DETECT}{LOGS_TAIL}");
    let payload = remote::stdin_payload(&script, &env);

    if a.dry_run {
        match &target {
            Target::Ssh(host) => println!(
                "ssh -T -o BatchMode=yes -o ConnectTimeout=15 -- {host} bash -s <<'KAI_EOF'"
            ),
            Target::Local => println!("bash -s <<'KAI_EOF'"),
        }
        print!("{payload}");
        println!("KAI_EOF");
        return Ok(ExitCode::SUCCESS);
    }

    if a.verbose {
        match &target {
            Target::Ssh(host) => eprintln!("[{}] docker logs через ssh {host}…", profile.name),
            Target::Local => eprintln!("[{}] docker logs локально…", profile.name),
        }
    }

    let status = match &target {
        Target::Ssh(host) => remote::run_via_stdin(host, &payload)
            .map_err(|e| AppError::Msg(format!("ssh: {e}")))?,
        Target::Local => {
            run_local(&payload).map_err(|e| AppError::Msg(format!("bash: {e}")))?
        }
    };
    let code = status.code().unwrap_or(1);
    print_hint(code, &a, &target);
    Ok(ExitCode::from(code.clamp(0, 255) as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_accepts_number_and_all() {
        assert!(validate_tail("500").is_ok());
        assert!(validate_tail("all").is_ok());
        assert!(validate_tail("").is_err());
        assert!(validate_tail("-1").is_err());
        assert!(validate_tail("500; rm -rf /").is_err());
    }

    #[test]
    fn when_rejects_flag_like_values() {
        assert!(validate_when("--since", "10m").is_ok());
        assert!(validate_when("--until", "2026-07-25T10:00:00Z").is_ok());
        assert!(validate_when("--since", "-f").is_err());
        assert!(validate_when("--since", "").is_err());
    }

    #[test]
    fn loopback_hosts_mean_local_docker() {
        assert!(is_loopback("localhost"));
        assert!(is_loopback("127.0.0.1"));
        assert!(!is_loopback("10.0.0.5"));
        assert!(!is_loopback("db.example.com"));
    }
}
