//! `sql-kai fork <alias> [имя]` — одноразовая локальная копия базы, чтобы гонять
//! миграции не на проде: снять дамп источника, поднять postgres в docker и
//! завести на него профиль. Идея из ghost.build («форкни базу и примени миграцию
//! на копии»), но без copy-on-write: базы у нас удалённые и чужие, снапшот тома
//! или файловой системы снять нечем — поэтому копия делается дампом, а «мгновенно»
//! получается только для схемы: она снимается по умолчанию, данные — по явному
//! `--data`.
//!
//! Источник строго read-only, и это обеспечено на всех путях: соединение
//! открывается через [`session::open_for`] с `write = false`, то есть сразу
//! получает `default_transaction_read_only = on`; единственная операция над
//! источником — `pg_dump` (SELECT-ы плюс ACCESS SHARE-локи, никакого DDL/DML).
//! Ни одна команда этого модуля ничего не пишет в исходную базу — всё, что
//! создаётся, создаётся локально: контейнер, дамп во временном файле и новый
//! профиль. Форк не наследует флаг `production`, чтобы GUI не переспрашивал на
//! копии то, ради чего копия и заводилась.
//!
//! Версия образа берётся из `SHOW server_version` источника: восстановить дамп
//! в чужой мажор нельзя (менялись и синтаксис, и системные каталоги), поэтому
//! `postgres:<мажор сервера>`; переопределяется через `--image` — например,
//! когда нужны расширения, которых нет в ванильном образе (postgis и т.п.).

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Output, Stdio};
use std::time::Duration;

use clap::Args;
use rand::Rng;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, Profile};
use sql_kai_lib::tunnel;
use sql_kai_lib::vault;

use crate::remote::{self, CONTAINER_DB_ENV, CONTAINER_FIND};
use crate::{sec, session};

#[derive(Args)]
pub struct ForkArgs {
    /// Профиль-источник: имя, id или группа
    alias: String,
    /// Имя профиля-форка (по умолчанию <профиль>-fork)
    new_name: Option<String>,
    /// То же, что позиционное имя форка (приоритетнее его)
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
    /// Копировать и данные (дамп может быть огромным; по умолчанию — только схема)
    #[arg(long)]
    data: bool,
    /// Docker-образ (по умолчанию postgres:<мажор версии сервера>)
    #[arg(long, value_name = "IMAGE")]
    image: Option<String>,
    /// Порт форка на 127.0.0.1 (по умолчанию свободный)
    #[arg(long)]
    port: Option<u16>,
    /// Docker-контейнер источника на ssh-хосте (когда их несколько)
    #[arg(long, value_name = "NAME")]
    container: Option<String>,
    /// Снести существующий контейнер форка и создать заново
    #[arg(long)]
    replace: bool,
    /// Не спрашивать подтверждение
    #[arg(long)]
    yes: bool,
    /// Показать план и выйти, ничего не делая
    #[arg(long)]
    dry_run: bool,
    /// Пароль источника из env-переменной (если не в vault/sec)
    #[arg(long, value_name = "VAR")]
    password_env: Option<String>,
    /// Пароль источника взять из sec
    #[arg(long)]
    from_sec: bool,
    /// Ключ sec для пароля источника (proj/KEY); включает --from-sec
    #[arg(long, value_name = "PROJ/KEY")]
    sec_key: Option<String>,
    #[arg(short, long)]
    verbose: bool,
}

/// Хвост поверх [`CONTAINER_FIND`] + [`CONTAINER_DB_ENV`]: `pg_dump` внутри контейнера-источника, дамп
/// уезжает в stdout ssh. Имя базы берём из профиля (`KAI_DB`), а не из env
/// контейнера: в одном кластере баз может быть несколько, а профиль знает нужную.
/// Роль — из контейнера (`$U`), она ходит по unix-сокету через trust, так что
/// пароль источника здесь не нужен вовсе.
const DUMP_TAIL: &str = r#"[ -n "${KAI_DB:-}" ] && DB="$KAI_DB"
if [ -n "${KAI_VERBOSE:-}" ]; then echo "[container=$C user=$U db=$DB]" >&2; fi
$D exec "$C" pg_dump -U "$U" -d "$DB" --no-owner --no-privileges ${KAI_DUMP_OPTS:-}
"#;

/// Сколько ждём, пока контейнер отработает initdb и начнёт слушать TCP.
const READY_TIMEOUT: Duration = Duration::from_secs(180);

/// `docker` без унаследованного мастер-пароля vault (см. [`vault::scrub_master_password_env`]).
fn docker() -> Command {
    let mut cmd = Command::new("docker");
    vault::scrub_master_password_env(&mut cmd);
    cmd
}

/// `docker <args>` с захватом вывода; отсутствие бинаря — отдельная ошибка,
/// иначе пользователь видит невнятное «No such file or directory».
fn docker_out(args: &[&str]) -> Result<Output, AppError> {
    docker().args(args).output().map_err(|e| match e.kind() {
        ErrorKind::NotFound => AppError::Msg(
            "docker не найден в PATH — поставь Docker Desktop (brew install --cask docker) \
             или colima"
                .into(),
        ),
        _ => AppError::Msg(format!("docker: {e}")),
    })
}

/// Docker установлен И демон отвечает — две разные беды с разными подсказками.
fn ensure_docker() -> Result<(), AppError> {
    let out = docker_out(&["version", "--format", "{{.Server.Version}}"])?;
    if out.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr)
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("демон не отвечает")
        .to_string();
    Err(AppError::Msg(format!(
        "docker есть, но не работает: {why}\nhint: запусти Docker Desktop (или `colima start`)"
    )))
}

/// Состояние контейнера (`running`/`exited`/…), None — контейнера нет.
fn container_state(name: &str) -> Result<Option<String>, AppError> {
    let out = docker_out(&["container", "inspect", "--format", "{{.State.Status}}", name])?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

/// Образ есть локально? (иначе тянем явно, чтобы «docker run» не молчал минуту)
fn image_present(image: &str) -> Result<bool, AppError> {
    Ok(docker_out(&["image", "inspect", "--format", "{{.Id}}", image])?
        .status
        .success())
}

fn pull_image(image: &str) -> Result<(), AppError> {
    println!("тяну образ {image}…");
    let status = docker()
        .args(["pull", image])
        .status()
        .map_err(|e| AppError::Msg(format!("docker pull: {e}")))?;
    if !status.success() {
        return Err(AppError::Msg(format!(
            "образ {image} не скачался — проверь, что такой тег существует, \
             или укажи свой через --image"
        )));
    }
    Ok(())
}

/// `postgres:<мажор>` под версию сервера. До 10-й мажор — две компоненты (9.6),
/// начиная с 10-й — одна (16).
fn image_for(server_version: &str) -> Result<String, AppError> {
    let digits: String = server_version
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = digits.split('.').filter(|s| !s.is_empty());
    let major: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            AppError::Msg(format!(
                "не разобрал версию сервера '{server_version}' — задай образ через --image"
            ))
        })?;
    if major >= 10 {
        Ok(format!("postgres:{major}"))
    } else {
        let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        Ok(format!("postgres:{major}.{minor}"))
    }
}

/// Имя контейнера: префикс + имя профиля, приведённое к алфавиту docker
/// ([a-zA-Z0-9_.-]). Префикс заодно даёт `docker ps | grep sql-kai-fork`.
fn container_name(fork_name: &str) -> String {
    let slug: String = fork_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("sql-kai-fork-{slug}")
}

/// base62 — пароль форка живёт в vault и в env контейнера, экранирование не нужно.
/// (тот же алфавит, что в `rotate`; там свой генератор ради sec-истории)
fn gen_password() -> String {
    const CS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..24).map(|_| CS[rng.gen_range(0..CS.len())] as char).collect()
}

/// Пароль источника для локального `pg_dump`: env → sec → vault. Тот же приоритет,
/// что у [`session::PwSource`], но здесь нужно само значение, а не override сессии.
fn source_password(profile: &Profile, a: &ForkArgs) -> Result<Option<String>, AppError> {
    if let Some(var) = a.password_env.as_deref() {
        return std::env::var(var)
            .map(Some)
            .map_err(|_| AppError::Msg(format!("env-переменная {var} не задана")));
    }
    if a.from_sec || a.sec_key.is_some() {
        sec::available()?;
        let key = a
            .sec_key
            .clone()
            .unwrap_or_else(|| sec::default_key(profile));
        return sec::get(&key)?
            .map(Some)
            .ok_or_else(|| AppError::Msg(format!("в sec нет ключа {key}")));
    }
    Ok(store::get_password(profile))
}

/// Пустой файл дампа с правами 0600 (в /tmp он может оказаться читаемым всем,
/// а с `--data` внутри лежат боевые данные).
fn create_dump_file(path: &Path) -> Result<(), AppError> {
    File::create(path).map_err(AppError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn open_dump_file(path: &Path) -> Result<File, AppError> {
    OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(AppError::Io)
}

/// Дамп через ssh + `docker exec pg_dump` — тот же путь, что у `discover`/`exec`
/// (общий [`CONTAINER_FIND`] + [`CONTAINER_DB_ENV`]). stdout уходит прямо в файл, а не в память:
/// дамп с `--data` бывает на гигабайты, и `remote::output_via_stdin` на нём
/// раздул бы процесс.
fn dump_remote(
    alias: &str,
    container: Option<&str>,
    database: &str,
    schema_only: bool,
    verbose: bool,
    out: &Path,
) -> Result<ExitStatus, AppError> {
    let script = format!("{CONTAINER_FIND}{CONTAINER_DB_ENV}{DUMP_TAIL}");
    let env = [
        ("KAI_CONTAINER", container.unwrap_or_default().to_string()),
        ("KAI_DB", database.to_string()),
        (
            "KAI_DUMP_OPTS",
            if schema_only { "--schema-only" } else { "" }.to_string(),
        ),
        ("KAI_VERBOSE", if verbose { "1" } else { "" }.to_string()),
    ];
    let payload = remote::stdin_payload(&script, &env);
    let file = open_dump_file(out)?;
    let mut child = remote::ssh_base(alias)
        .args(["bash", "-s"])
        .stdin(Stdio::piped())
        .stdout(Stdio::from(file))
        .spawn()
        .map_err(|e| AppError::Msg(format!("ssh: {e}")))?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(payload.as_bytes())
        .map_err(AppError::Io)?;
    child.wait().map_err(AppError::Io)
}

/// Дамп локальным `pg_dump` — для профилей без ssh и как фолбэк, когда на
/// ssh-хосте нет docker-контейнера (bastion → RDS): ходим в уже поднятый туннель.
fn dump_local(
    profile: &Profile,
    endpoint: (&str, u16),
    password: Option<&str>,
    schema_only: bool,
    out: &Path,
) -> Result<ExitStatus, AppError> {
    let mut cmd = Command::new("pg_dump");
    // -w: без пароля падать, а не виснуть на промпте (BatchMode ssh-обвязки)
    cmd.args(["--no-owner", "--no-privileges", "-w"]);
    if schema_only {
        cmd.arg("--schema-only");
    }
    cmd.arg("-h")
        .arg(endpoint.0)
        .arg("-p")
        .arg(endpoint.1.to_string())
        .arg("-U")
        .arg(&profile.user)
        .arg("-d")
        .arg(&profile.database)
        .env("PGCONNECT_TIMEOUT", "10");
    if let Some(pw) = password {
        cmd.env("PGPASSWORD", pw);
    }
    if profile.ssl.as_ref().is_some_and(|s| s.enabled) {
        cmd.env("PGSSLMODE", "require");
    }
    vault::scrub_master_password_env(&mut cmd);
    let file = open_dump_file(out)?;
    cmd.stdout(Stdio::from(file));
    cmd.status().map_err(|e| match e.kind() {
        ErrorKind::NotFound => AppError::Msg(
            "pg_dump не найден в PATH — поставь клиент postgres \
             (brew install libpq && brew link --force libpq)"
                .into(),
        ),
        _ => AppError::Msg(format!("pg_dump: {e}")),
    })
}

/// Ждём, пока контейнер отработает initdb: во время инициализации postgres в
/// официальном образе поднят с `listen_addresses=''`, поэтому проверяем именно
/// TCP внутри контейнера, а не unix-сокет и не проброшенный порт (тот слушает
/// docker-proxy с первой секунды и «готовностью» не является).
async fn wait_ready(cname: &str, user: &str, database: &str) -> Result<(), AppError> {
    let deadline = std::time::Instant::now() + READY_TIMEOUT;
    loop {
        match container_state(cname)?.as_deref() {
            Some("running") => {}
            Some(other) => {
                return Err(AppError::Msg(format!(
                    "контейнер {cname} в состоянии '{other}' — смотри `docker logs {cname}`"
                )))
            }
            None => {
                return Err(AppError::Msg(format!(
                    "контейнер {cname} исчез — смотри `docker logs {cname}`"
                )))
            }
        }
        let ok = docker_out(&[
            "exec", cname, "pg_isready", "-h", "127.0.0.1", "-p", "5432", "-U", user, "-d",
            database,
        ])?
        .status
        .success();
        if ok {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(AppError::Msg(format!(
                "контейнер {cname} не поднялся за {} с — смотри `docker logs {cname}`",
                READY_TIMEOUT.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Заливка дампа в форк: psql внутри контейнера (unix-сокет, trust — пароль не
/// нужен). Схему льём одной транзакцией (или вся, или ничего), данные — нет:
/// на большом дампе одна транзакция раздувает WAL.
fn restore(cname: &str, user: &str, database: &str, dump: &Path, schema_only: bool) -> Result<ExitStatus, AppError> {
    let file = File::open(dump).map_err(AppError::Io)?;
    let mut cmd = docker();
    cmd.args(["exec", "-i", cname, "psql", "-U", user, "-d", database])
        // -q глушит только command-теги; дамп зовёт ещё и SELECT'ы
        // (`set_config`, `setval` у сиквенсов), а их таблички результатов
        // печатались прямо в вывод команды. -o шлёт результаты запросов в
        // никуда, ошибки при этом остаются на stderr.
        .args(["-v", "ON_ERROR_STOP=1", "-q", "-o", "/dev/null"]);
    if schema_only {
        cmd.arg("--single-transaction");
    }
    cmd.args(["-f", "-"]).stdin(Stdio::from(file));
    cmd.status().map_err(|e| AppError::Msg(format!("docker exec psql: {e}")))
}

/// Сколько обычных таблиц приехало — дешёвая проверка, что форк не пустой.
fn count_tables(cname: &str, user: &str, database: &str) -> Option<String> {
    const SQL: &str = "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
        WHERE c.relkind IN ('r','p') AND n.nspname NOT IN ('pg_catalog','information_schema')";
    let out = docker_out(&["exec", cname, "psql", "-U", user, "-d", database, "-tAc", SQL]).ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub async fn run(a: ForkArgs) -> Result<ExitCode, AppError> {
    // Docker проверяем до коннекта: незачем дёргать прод, если форк всё равно
    // некуда положить.
    ensure_docker()?;

    // write = false: сессия к источнику read-only на весь путь (см. докблок).
    let (profile, connected) = session::open_for(
        &a.alias,
        session::PwSource {
            env: a.password_env.as_deref(),
            from_sec: a.from_sec,
            sec_key: a.sec_key.as_deref(),
        },
        false,
        a.verbose,
        true,
    )
    .await?;

    let fork_name = a
        .name
        .clone()
        .or_else(|| a.new_name.clone())
        .unwrap_or_else(|| format!("{}-fork", profile.name));
    if fork_name.eq_ignore_ascii_case(&profile.name) {
        return Err(AppError::Msg(format!(
            "имя форка совпадает с именем источника ('{fork_name}') — форк затёр бы профиль-источник"
        )));
    }
    // Повторный fork обновляет свой профиль, но чужой (не форк) затирать нельзя:
    // у него другой host/ssh, и пользователь потерял бы рабочее подключение.
    let existing_profile = store::load_profiles()?
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(&fork_name));
    if let Some(e) = &existing_profile {
        if e.ssh.is_some() || e.host != "127.0.0.1" {
            return Err(AppError::Msg(format!(
                "профиль '{}' уже есть и указывает не на локальный форк ({}:{}) — \
                 выбери другое имя (--name)",
                e.name, e.host, e.port
            )));
        }
    }
    let image = match &a.image {
        Some(i) => i.clone(),
        None => image_for(&connected.server_version)?,
    };
    let cname = container_name(&fork_name);
    let existing_container = container_state(&cname)?;
    if existing_container.is_some() && !a.replace {
        return Err(AppError::Msg(format!(
            "контейнер {cname} уже существует — снеси его (`docker rm -f {cname}`) \
             или пересоздай форк с --replace"
        )));
    }
    let port = match a.port {
        Some(p) => {
            // Занятый порт docker обнаружит только на run — сообщим раньше и понятнее.
            std::net::TcpListener::bind(("127.0.0.1", p))
                .map_err(|e| AppError::Msg(format!("порт {p} занять не выйдет: {e}")))?;
            p
        }
        None => tunnel::free_port()?,
    };
    let via_ssh = profile
        .ssh
        .as_ref()
        .is_some_and(|s| !s.host.trim().is_empty());

    println!("что будет сделано:");
    println!(
        "  источник   {} ({}:{}/{}, PostgreSQL {}) — только чтение",
        profile.name,
        profile.host,
        profile.port,
        profile.database,
        if connected.server_version.is_empty() {
            "?"
        } else {
            &connected.server_version
        }
    );
    println!(
        "  дамп       pg_dump {} {}",
        if a.data {
            "со всеми данными (может быть долго и на гигабайты)"
        } else {
            "только схема (--data — вместе с данными)"
        },
        if via_ssh {
            format!("через ssh {} + docker exec", profile.ssh.as_ref().map(|s| s.host.clone()).unwrap_or_default())
        } else {
            "локальным pg_dump".to_string()
        }
    );
    println!("  контейнер  docker run {image} -> {cname} на 127.0.0.1:{port}");
    if existing_container.is_some() {
        println!("             (--replace: прежний контейнер {cname} будет удалён вместе с данными)");
    }
    println!(
        "  профиль    '{fork_name}'{} -> 127.0.0.1:{port}/{}, пароль в vault",
        if existing_profile.is_some() { " (существующий обновится)" } else { "" },
        profile.database
    );
    if a.data {
        println!("  на источнике pg_dump держит ACCESS SHARE-локи: DDL/VACUUM FULL на нём подождут");
    }

    if a.dry_run {
        println!("dry-run: ничего не сделано");
        return Ok(ExitCode::SUCCESS);
    }
    if !a.yes && !crate::input::confirm("продолжить?", "--yes")? {
        eprintln!("отменено");
        return Ok(ExitCode::FAILURE);
    }

    if !image_present(&image)? {
        pull_image(&image)?;
    }

    let dump_path: PathBuf =
        std::env::temp_dir().join(format!("{cname}-{}.sql", std::process::id()));
    create_dump_file(&dump_path)?;
    eprintln!("снимаю дамп…");
    let mut dumped = false;
    if via_ssh {
        let alias = profile.ssh.as_ref().map(|s| s.host.clone()).unwrap_or_default();
        let status = dump_remote(
            &alias,
            a.container.as_deref(),
            &profile.database,
            !a.data,
            a.verbose,
            &dump_path,
        )?;
        match status.code() {
            Some(0) => dumped = true,
            // 3/5 — коды CONTAINER_FIND/CONTAINER_DB_ENV: postgres на хосте не в docker
            // (bastion -> RDS/облако). Дамп снимем локально через туннель.
            Some(3) | Some(5) => eprintln!(
                "sql-kai: postgres-контейнер на '{alias}' не найден — снимаю дамп \
                 локальным pg_dump через туннель"
            ),
            Some(c) => {
                return Err(AppError::Msg(format!(
                    "pg_dump на хосте вернул код {c} (дамп: {})",
                    dump_path.display()
                )))
            }
            None => return Err(AppError::Msg("ssh прерван сигналом".into())),
        }
    }
    if !dumped {
        let endpoint = match connected.tunnel_port {
            Some(p) => ("127.0.0.1".to_string(), p),
            None => (profile.host.clone(), profile.port),
        };
        let password = source_password(&profile, &a)?;
        let status = dump_local(
            &profile,
            (&endpoint.0, endpoint.1),
            password.as_deref(),
            !a.data,
            &dump_path,
        )?;
        if !status.success() {
            return Err(AppError::Msg(format!(
                "pg_dump не снял дамп (код {})",
                status.code().unwrap_or(-1)
            )));
        }
    }
    // Источник больше не нужен: рвём соединение и ssh-туннель до того, как
    // начнём возиться с контейнером (это минуты).
    drop(connected);

    let dump_size = std::fs::metadata(&dump_path).map(|m| m.len()).unwrap_or(0);
    if dump_size == 0 {
        let _ = std::fs::remove_file(&dump_path);
        return Err(AppError::Msg(
            "дамп пустой — pg_dump ничего не вернул (проверь права роли на источнике)".into(),
        ));
    }
    eprintln!("дамп: {} КиБ", dump_size / 1024);

    if existing_container.is_some() {
        let out = docker_out(&["rm", "-f", &cname])?;
        if !out.status.success() {
            return Err(AppError::Msg(format!(
                "не удалось удалить прежний контейнер {cname}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }

    let fork_password = gen_password();
    // POSTGRES_PASSWORD передаём наследованием env (`-e VAR` без значения):
    // в argv он попал бы в `ps` любому пользователю машины.
    // Порт публикуем только на 127.0.0.1 — копия прода наружу не торчит.
    let out = docker()
        .args(["run", "-d", "--name", &cname])
        .args(["-e", "POSTGRES_PASSWORD"])
        .args(["-e", &format!("POSTGRES_USER={}", profile.user)])
        .args(["-e", &format!("POSTGRES_DB={}", profile.database)])
        .args(["-p", &format!("127.0.0.1:{port}:5432")])
        .args(["--label", &format!("sql-kai.fork={}", profile.name)])
        .arg(&image)
        .env("POSTGRES_PASSWORD", &fork_password)
        .output()
        .map_err(|e| AppError::Msg(format!("docker run: {e}")))?;
    if !out.status.success() {
        return Err(AppError::Msg(format!(
            "docker run не поднял контейнер: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    eprintln!("контейнер {cname} запущен, жду готовности…");
    wait_ready(&cname, &profile.user, &profile.database).await?;

    eprintln!("заливаю дамп…");
    let status = restore(&cname, &profile.user, &profile.database, &dump_path, !a.data)?;
    if !status.success() {
        eprintln!(
            "sql-kai: psql не залил дамп (код {})",
            status.code().unwrap_or(-1)
        );
        eprintln!("дамп остался: {}", dump_path.display());
        eprintln!(
            "hint: если упало на CREATE EXTENSION — нужен образ с этим расширением: \
             sql-kai fork {} --image <образ> --replace",
            a.alias
        );
        eprintln!("hint: контейнер оставлен для разбора — `docker logs {cname}`, снести: `docker rm -f {cname}`");
        return Ok(ExitCode::FAILURE);
    }
    let _ = std::fs::remove_file(&dump_path);

    // Профиль форка: тот же id, если такой уже есть (повторный fork обновляет
    // порт), группу/цвет существующего не теряем. production не наследуем
    // никогда — копия для того и заведена, чтобы на ней можно было писать.
    let existing = existing_profile.as_ref();
    let fork_profile = Profile {
        id: existing.map(|e| e.id.clone()).unwrap_or_default(),
        name: fork_name.clone(),
        host: "127.0.0.1".into(),
        port,
        database: profile.database.clone(),
        user: profile.user.clone(),
        ssh: None,
        group: existing.and_then(|e| e.group.clone()),
        color: existing.and_then(|e| e.color.clone()),
        production: false,
        ssl: None,
        has_password: existing.map(|e| e.has_password).unwrap_or(false),
        has_ssh_passphrase: false,
        last_connected: None,
    };
    let saved = match session::unlock_vault() {
        Ok(()) => Some(store::upsert_profile(fork_profile, Some(fork_password.clone()), None)?),
        Err(e) => {
            eprintln!("sql-kai: vault не открылся ({e}) — профиль форка не сохранён");
            eprintln!(
                "пароль контейнера {cname}: {fork_password} (положи его сам или пересоздай форк)"
            );
            None
        }
    };
    if let Some(p) = &saved {
        crate::broker_client::notify_profiles_changed().await;
        println!("готово: профиль '{}' -> 127.0.0.1:{}/{}", p.name, p.port, p.database);
    }
    if let Some(n) = count_tables(&cname, &profile.user, &profile.database) {
        println!("таблиц в форке: {n}{}", if a.data { "" } else { " (без данных)" });
    }
    println!("  миграция на копии: sql-kai {fork_name} -f migration.sql --write");
    println!("  удалить форк:      docker rm -f {cname}");
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::{container_name, image_for};

    #[test]
    fn image_by_server_version() {
        assert_eq!(image_for("16.4 (Debian 16.4-1.pgdg120+1)").unwrap(), "postgres:16");
        assert_eq!(image_for("17.2").unwrap(), "postgres:17");
        assert_eq!(image_for("9.6.24").unwrap(), "postgres:9.6");
        assert!(image_for("").is_err());
        assert!(image_for("unknown").is_err());
    }

    #[test]
    fn container_name_is_docker_safe() {
        assert_eq!(container_name("vuln-fork"), "sql-kai-fork-vuln-fork");
        assert_eq!(container_name("ms search/2"), "sql-kai-fork-ms-search-2");
    }
}
