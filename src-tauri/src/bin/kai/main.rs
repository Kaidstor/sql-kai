//! kai — CLI к sql-kai: выполняет SQL в базах из профилей приложения (общие
//! profiles.json / vault / история с GUI), заводит профили дискавери по ssh
//! (`kai discover`) и умеет прямой fallback через ssh+docker exec (`kai exec`).
//!
//! Сессия по умолчанию read-only (`SET default_transaction_read_only = on`);
//! запись — только с явным `--write`. Чувствительные колонки (password/
//! secret/*_token/*_key) в выводе маскируются; отключение — `--no-redact`.

mod broker_client;
mod discover;
mod doctor;
mod execmode;
mod output;
mod redact;
mod remote;
mod rotate;
mod sec;
mod session;

use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use sql_tauri_lib::db;
use sql_tauri_lib::error::AppError;
use sql_tauri_lib::store::{self, HistoryEntry, Profile};
use sql_tauri_lib::tunnel;
use sql_tauri_lib::vault;

use output::Format;

#[derive(Parser)]
#[command(
    name = "kai",
    version,
    about = "SQL к Postgres по профилям sql-kai (ssh-туннели, vault, история)",
    after_help = "Примеры:\n  \
        kai domainator -c \"SELECT count(*) FROM domains\"\n  \
        echo \"SELECT now()\" | kai orchestrator --json\n  \
        kai discover coordinator       # ssh-хост -> профиль (+ пароль в vault)\n  \
        kai exec coordinator -c \"SELECT 1\"   # fallback: ssh + docker exec psql\n  \
        kai tables orchestrator --counts     # таблицы + примерное число строк\n  \
        kai vault trust                # тихий доступ CLI к паролям vault"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Выполнить SQL в базе профиля (kai <alias> — то же самое)
    #[command(alias = "query")]
    Q(QueryArgs),
    /// Прямой режим без профиля: ssh <alias> -> docker exec psql
    Exec(execmode::ExecArgs),
    /// Найти postgres на ssh-хосте и создать/обновить профиль
    Discover(DiscoverArgs),
    /// Профили (общие с GUI)
    Profiles {
        #[command(subcommand)]
        cmd: Option<ProfilesCmd>,
    },
    /// Список таблиц/вьюх базы
    Tables(TablesArgs),
    /// Колонки таблицы
    Columns(TableArgs),
    /// DDL таблицы (CREATE TABLE / VIEW)
    Ddl(TableArgs),
    /// Индексы таблицы
    Indexes(TableArgs),
    /// История выполненных запросов
    History(HistoryArgs),
    /// Сохранённые запросы (общие с GUI)
    Saved {
        #[command(subcommand)]
        cmd: SavedCmd,
    },
    /// Ротация пароля роли Postgres (sec + ALTER ROLE, старое в истории sec)
    Rotate(RotateArgs),
    /// Здоровье соединений: сохранённые пароли ещё аутентифицируются?
    Doctor(DoctorArgs),
    /// Живые сессии запущенного GUI: его собственные и cli-сессии брокера
    Sessions(SessionsArgs),
    /// Персистентные ssh-туннели (ControlMaster), переиспользуемые между вызовами
    Tunnel {
        #[command(subcommand)]
        cmd: Option<TunnelCmd>,
    },
    /// Vault с паролями БД
    Vault {
        #[command(subcommand)]
        cmd: VaultCmd,
    },
}

#[derive(Args)]
struct FormatArgs {
    /// JSON-вывод (columns/rows/rowsAffected/truncated); значения приведены
    /// к типам колонок БД: числа/bool/null/json (точное строковое
    /// представление — кастуй в ::text)
    #[arg(long, group = "fmt")]
    json: bool,
    /// CSV-вывод
    #[arg(long, group = "fmt")]
    csv: bool,
    /// tuples-only: значения через |, без заголовков
    #[arg(short = 't', long, group = "fmt")]
    tuples: bool,
}

impl FormatArgs {
    fn pick(&self) -> Format {
        if self.json {
            Format::Json
        } else if self.csv {
            Format::Csv
        } else if self.tuples {
            Format::Tuples
        } else {
            Format::Table
        }
    }
}

#[derive(Args)]
struct QueryArgs {
    /// Профиль: имя, id или группа
    alias: String,
    /// SQL-команда (можно повторять)
    #[arg(short = 'c', long = "command", value_name = "SQL")]
    commands: Vec<String>,
    /// Файл с SQL (можно повторять)
    #[arg(short = 'f', long = "file", value_name = "FILE")]
    files: Vec<PathBuf>,
    #[command(flatten)]
    fmt: FormatArgs,
    /// Максимум строк на результат
    #[arg(long, default_value_t = 1000, value_name = "N")]
    max_rows: usize,
    /// Разрешить запись (по умолчанию сессия read-only)
    #[arg(long)]
    write: bool,
    /// Env-переменная с паролем БД (обход vault)
    #[arg(long, value_name = "VAR")]
    password_env: Option<String>,
    /// Взять пароль БД из sec (ключ <имя>/DB_PASSWORD)
    #[arg(long)]
    from_sec: bool,
    /// Ключ sec для пароля (proj/KEY); включает --from-sec
    #[arg(long, value_name = "PROJ/KEY")]
    sec_key: Option<String>,
    /// Не записывать запрос в историю
    #[arg(long)]
    no_history: bool,
    /// Не маскировать чувствительные колонки (password/secret/*_token/*_key)
    #[arg(long)]
    no_redact: bool,
    /// Не переиспользовать ssh-туннель (без ControlMaster)
    #[arg(long)]
    no_mux: bool,
    /// Не ходить через брокер запущенного GUI — всегда своя сессия
    #[arg(long)]
    local: bool,
    /// Показать, куда подключились
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Args)]
struct SessionsArgs {
    /// JSON-вывод
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct DiscoverArgs {
    /// SSH-алиас хоста (из ~/.ssh/config*)
    alias: String,
    /// Имя профиля (по умолчанию = alias)
    #[arg(long)]
    name: Option<String>,
    /// Положить найденный пароль в sec (по умолч. ключ <имя>/DB_PASSWORD)
    #[arg(long)]
    to_sec: bool,
    /// Ключ sec для пароля (proj/KEY); включает --to-sec
    #[arg(long, value_name = "PROJ/KEY")]
    sec_key: Option<String>,
    /// Не хранить пароль в vault (прод-политика: только sec / --password-env)
    #[arg(long)]
    no_store: bool,
    /// Только показать найденное, не сохранять профиль
    #[arg(long)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum ProfilesCmd {
    /// Список профилей
    List {
        #[arg(long)]
        json: bool,
    },
    /// Показать профиль
    Show { alias: String },
    /// Удалить профиль (и его секреты из vault)
    Rm {
        alias: String,
        /// Не спрашивать подтверждение
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
struct TablesArgs {
    /// Профиль: имя, id или группа
    alias: String,
    /// Примерное число строк (pg_class.reltuples; '?' = не анализировалась)
    #[arg(long)]
    counts: bool,
    #[arg(long)]
    json: bool,
    #[arg(long, value_name = "VAR")]
    password_env: Option<String>,
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Args)]
struct TableArgs {
    /// Профиль: имя, id или группа
    alias: String,
    /// Таблица: [schema.]table (по умолчанию схема public)
    table: String,
    #[arg(long)]
    json: bool,
    #[arg(long, value_name = "VAR")]
    password_env: Option<String>,
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Args)]
struct HistoryArgs {
    /// Фильтр по профилю (имя или id)
    alias: Option<String>,
    /// Сколько записей показать
    #[arg(short = 'n', long, default_value_t = 20)]
    limit: usize,
    #[arg(long)]
    json: bool,
    /// Прогнать history.json через `sec scan` — не утёк ли секрет в открытом виде
    #[arg(long)]
    scan: bool,
}

#[derive(Subcommand)]
enum SavedCmd {
    /// Сохранённые запросы: глобальные + видимые профилю
    List {
        alias: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Выполнить сохранённый запрос по имени
    Run {
        /// Профиль: имя, id или группа
        alias: String,
        /// Имя сохранённого запроса
        name: String,
        #[command(flatten)]
        fmt: FormatArgs,
        #[arg(long, default_value_t = 1000, value_name = "N")]
        max_rows: usize,
        /// Разрешить запись (по умолчанию сессия read-only)
        #[arg(long)]
        write: bool,
        #[arg(long, value_name = "VAR")]
        password_env: Option<String>,
        /// Взять пароль БД из sec (ключ <имя>/DB_PASSWORD)
        #[arg(long)]
        from_sec: bool,
        /// Ключ sec для пароля (proj/KEY); включает --from-sec
        #[arg(long, value_name = "PROJ/KEY")]
        sec_key: Option<String>,
        /// Не маскировать чувствительные колонки (password/secret/*_token/*_key)
        #[arg(long)]
        no_redact: bool,
        /// Не переиспользовать ssh-туннель (без ControlMaster)
        #[arg(long)]
        no_mux: bool,
        #[arg(short, long)]
        verbose: bool,
    },
}

#[derive(Args)]
struct RotateArgs {
    /// Профиль: имя, id или группа
    alias: String,
    /// Роль Postgres (по умолчанию — пользователь профиля)
    #[arg(long)]
    role: Option<String>,
    /// Ключ sec для пароля (proj/KEY); по умолчанию <имя>/DB_PASSWORD
    #[arg(long, value_name = "PROJ/KEY")]
    sec_key: Option<String>,
    /// Длина нового пароля
    #[arg(long, default_value_t = 32)]
    length: usize,
    /// Интервал ротации для sec meta (напр. 90d)
    #[arg(long, value_name = "DUR")]
    rotate_every: Option<String>,
    /// Текущий пароль из env-переменной (если не в vault/sec)
    #[arg(long, value_name = "VAR")]
    password_env: Option<String>,
    /// Текущий пароль взять из sec
    #[arg(long)]
    from_sec: bool,
    /// Не спрашивать подтверждение
    #[arg(long)]
    yes: bool,
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Args)]
struct DoctorArgs {
    /// Проверить один профиль (имя или id); без него — все
    alias: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum TunnelCmd {
    /// Показать живые ssh-мастера (по умолчанию)
    List,
    /// Закрыть мастер(а): по хосту/имени сокета или все
    Close {
        /// Хост или имя сокета; без него — все
        target: Option<String>,
        /// Закрыть все
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum VaultCmd {
    /// Есть ли vault, Touch ID, CLI-trust
    Status,
    /// Создать vault (мастер-пароль)
    Setup,
    /// Разрешить CLI тихий доступ к vault через keychain
    Trust,
    /// Отозвать CLI-доступ
    Revoke,
}

/// `kai <alias> -c ...` — шорткат для `kai q <alias> -c ...`: если первый
/// аргумент не подкоманда и не флаг, считаем его алиасом профиля.
fn preprocess_args() -> Vec<OsString> {
    use clap::CommandFactory;
    // Single source of truth: derive the subcommand names (+ aliases) straight
    // from the clap definition, so a new `Cmd` variant can never be silently
    // misread as a profile alias.
    let cmd = Cli::command();
    let known: Vec<String> = cmd
        .get_subcommands()
        .flat_map(|c| {
            std::iter::once(c.get_name().to_string())
                .chain(c.get_all_aliases().map(str::to_string))
        })
        .chain(std::iter::once("help".to_string()))
        .collect();
    let mut args: Vec<OsString> = std::env::args_os().collect();
    if let Some(first) = args.get(1) {
        let s = first.to_string_lossy();
        if !s.starts_with('-') && !known.iter().any(|k| k == s.as_ref()) {
            args.insert(1, "q".into());
        }
    }
    args
}

fn main() -> ExitCode {
    let cli = Cli::parse_from(preprocess_args());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    match rt.block_on(dispatch(cli)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("kai: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn dispatch(cli: Cli) -> Result<ExitCode, AppError> {
    match cli.cmd {
        Cmd::Q(a) => cmd_query(a).await,
        Cmd::Exec(a) => execmode::run(a),
        Cmd::Discover(a) => discover::run(a).await,
        Cmd::Profiles { cmd } => cmd_profiles(cmd.unwrap_or(ProfilesCmd::List { json: false })),
        Cmd::Tables(a) => cmd_tables(a).await,
        Cmd::Columns(a) => cmd_table_info(a, TableInfoKind::Columns).await,
        Cmd::Ddl(a) => cmd_table_info(a, TableInfoKind::Ddl).await,
        Cmd::Indexes(a) => cmd_table_info(a, TableInfoKind::Indexes).await,
        Cmd::History(a) => cmd_history(a),
        Cmd::Saved { cmd } => cmd_saved(cmd).await,
        Cmd::Rotate(a) => rotate::run(a).await,
        Cmd::Doctor(a) => doctor::run(a).await,
        Cmd::Sessions(a) => cmd_sessions(a).await,
        Cmd::Tunnel { cmd } => cmd_tunnel(cmd.unwrap_or(TunnelCmd::List)),
        Cmd::Vault { cmd } => cmd_vault(cmd),
    }
}

// --- q ------------------------------------------------------------------------

/// Общий рендер результата: маскирование, формат (+типизированный json),
/// verbose-времена. `types` пустой, когда формат не json.
fn render_exec(a: &QueryArgs, mut exec: db::ExecResult, types: &[Option<Vec<(String, db::Type)>>]) {
    if !a.no_redact {
        let masked = redact::redact_exec(&mut exec);
        if !masked.is_empty() {
            eprintln!(
                "⚠ kai: маскированы чувствительные колонки: {} (показать: --no-redact)",
                masked.join(", ")
            );
        }
    }
    let fmt = a.fmt.pick();
    if fmt == Format::Json {
        let untyped = output::print_exec_json(&exec, types);
        if untyped > 0 {
            eprintln!(
                "⚠ kai: не удалось определить типы колонок для {untyped} стейтмент(а/ов) — их значения строками"
            );
        }
    } else {
        output::print_exec(&exec, fmt);
    }
    if a.verbose {
        eprintln!("({} ms)", exec.duration_ms);
    }
}

/// (имя, oid) с провода → типы для print_exec_json; незнакомый oid (кастомный
/// enum и т.п.) выводится как text — так же он выглядит и в автономном пути.
fn wire_types(
    wire: sql_tauri_lib::broker::WireColumnTypes,
) -> Vec<Option<Vec<(String, db::Type)>>> {
    wire.into_iter()
        .map(|cols| {
            cols.map(|cols| {
                cols.into_iter()
                    .map(|(name, oid)| (name, db::Type::from_oid(oid).unwrap_or(db::Type::TEXT)))
                    .collect()
            })
        })
        .collect()
}

/// Пытается обслужить запрос брокером запущенного GUI. Some(exit) — запрос
/// выполнен (или окончательно отвергнут) брокером; None — GUI недоступен или
/// vault заперт, и нужно идти автономным путём.
async fn try_broker_query(a: &QueryArgs, sql: &str) -> Result<Option<ExitCode>, AppError> {
    let Some(mut b) = broker_client::connect().await else {
        return Ok(None);
    };
    let profile = session::resolve_profile(&a.alias)?;
    if a.verbose {
        eprintln!("→ через брокер GUI {} (сессию держит приложение)", b.hello.server_version);
    }
    let with_types = a.fmt.pick() == Format::Json;
    let outcome = tokio::select! {
        r = b.query(&profile.id, sql, a.max_rows.max(1), a.write, with_types) => r,
        _ = tokio::signal::ctrl_c() => {
            // наш сокет занят ожиданием ответа — отмену шлём новым соединением
            if let Some(mut c) = broker_client::connect().await {
                let _ = c.cancel(&profile.id).await;
            }
            eprintln!("kai: отменено");
            return Ok(Some(ExitCode::FAILURE));
        }
    };
    let record = |ok: bool| {
        if !a.no_history {
            let _ = store::record_history(HistoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                profile_id: profile.id.clone(),
                profile_name: profile.name.clone(),
                sql: sql.to_string(),
                at: session::now_ms(),
                ok,
            });
        }
    };
    match outcome {
        Ok(res) => {
            record(true);
            let types = res.column_types.map(wire_types).unwrap_or_default();
            render_exec(a, res.exec, &types);
            Ok(Some(ExitCode::SUCCESS))
        }
        Err(broker_client::BrokerError::Query(msg)) => {
            record(false);
            eprintln!("kai: {msg}");
            if msg.contains("read-only") {
                eprintln!("hint: сессия read-only по умолчанию — повтори с --write, если изменение согласовано");
            }
            Ok(Some(ExitCode::FAILURE))
        }
        Err(e) => {
            if a.verbose {
                eprintln!("… брокер недоступен ({e}) — автономный режим");
            }
            Ok(None)
        }
    }
}

async fn cmd_query(a: QueryArgs) -> Result<ExitCode, AppError> {
    let sql = session::collect_sql(&a.commands, &a.files)?;
    // Живой GUI обслуживает запрос своей cli-сессией; кастомные источники
    // пароля (--password-env/--from-sec) — всегда автономно.
    if !a.local && a.password_env.is_none() && !a.from_sec && a.sec_key.is_none() {
        if let Some(code) = try_broker_query(&a, &sql).await? {
            return Ok(code);
        }
    }
    let (profile, connected) = session::open_for(
        &a.alias,
        session::PwSource {
            env: a.password_env.as_deref(),
            from_sec: a.from_sec,
            sec_key: a.sec_key.as_deref(),
        },
        a.write,
        a.verbose,
        !a.no_mux,
    )
    .await?;

    let outcome = db::execute(&connected.session.client, &sql, a.max_rows.max(1)).await;
    if !a.no_history {
        let _ = store::record_history(HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            sql: sql.clone(),
            at: session::now_ms(),
            ok: outcome.is_ok(),
        });
    }
    match outcome {
        Ok(exec) => {
            let types = if a.fmt.pick() == Format::Json {
                db::statement_column_types(&connected.session.client, &sql).await
            } else {
                Vec::new()
            };
            render_exec(&a, exec, &types);
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            eprintln!("kai: {e}");
            if e.to_string().contains("read-only") {
                eprintln!("hint: сессия read-only по умолчанию — повтори с --write, если изменение согласовано");
            }
            Ok(ExitCode::FAILURE)
        }
    }
}

// --- sessions -------------------------------------------------------------------

/// Живые сессии запущенного GUI: собственные (origin=gui) и cli-сессии
/// брокера (origin=cli, с простоем).
async fn cmd_sessions(a: SessionsArgs) -> Result<ExitCode, AppError> {
    let Some(mut b) = broker_client::connect().await else {
        eprintln!("GUI не запущен — брокер недоступен (kai работает автономно)");
        return Ok(ExitCode::FAILURE);
    };
    let list = b
        .sessions()
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?;
    if a.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".into())
        );
        return Ok(ExitCode::SUCCESS);
    }
    if list.is_empty() {
        println!("живых сессий нет (gui {})", b.hello.server_version);
        return Ok(ExitCode::SUCCESS);
    }
    println!("{:<7} {:<28} {:<7} {:<8} {}", "ORIGIN", "PROFILE", "TX", "TUNNEL", "IDLE");
    for s in list {
        println!(
            "{:<7} {:<28} {:<7} {:<8} {}",
            s.origin,
            s.profile_name,
            s.tx,
            s.tunnel_port
                .map(|p| format!(":{p}"))
                .unwrap_or_else(|| "-".into()),
            s.idle_sec
                .map(|i| format!("{i}s"))
                .unwrap_or_else(|| "-".into()),
        );
    }
    Ok(ExitCode::SUCCESS)
}

// --- introspection --------------------------------------------------------------

async fn cmd_tables(a: TablesArgs) -> Result<ExitCode, AppError> {
    let (_, connected) = session::open_for(
        &a.alias,
        session::PwSource {
            env: a.password_env.as_deref(),
            ..Default::default()
        },
        false,
        a.verbose,
        true,
    )
    .await?;
    let sql = if a.counts { db::TABLES_COUNTS_SQL } else { db::TABLES_SQL };
    let rows = db::query_rows(&connected.session.client, sql).await?;
    let mapped: Vec<Vec<Option<String>>> = rows
        .into_iter()
        .map(|row| {
            let kind = match row.get(2).and_then(|v| v.as_deref()) {
                Some("v") => "view",
                Some("m") => "matview",
                Some("f") => "foreign",
                _ => "table",
            };
            let mut out = vec![row[0].clone(), row[1].clone(), Some(kind.to_string())];
            if a.counts {
                out.push(row.get(3).cloned().flatten());
            }
            out
        })
        .collect();
    let headers: &[&str] = if a.counts {
        &["schema", "name", "kind", "~rows"]
    } else {
        &["schema", "name", "kind"]
    };
    output::print_rows(headers, &mapped, a.json);
    Ok(ExitCode::SUCCESS)
}

enum TableInfoKind {
    Columns,
    Ddl,
    Indexes,
}

/// "schema.table" -> (schema, table); без точки — public.
fn split_table(spec: &str) -> (String, String) {
    match spec.split_once('.') {
        Some((s, t)) => (s.to_string(), t.to_string()),
        None => ("public".to_string(), spec.to_string()),
    }
}

async fn cmd_table_info(a: TableArgs, kind: TableInfoKind) -> Result<ExitCode, AppError> {
    let (schema, table) = split_table(&a.table);
    let (_, connected) = session::open_for(
        &a.alias,
        session::PwSource {
            env: a.password_env.as_deref(),
            ..Default::default()
        },
        false,
        a.verbose,
        true,
    )
    .await?;
    let client = &connected.session.client;
    match kind {
        TableInfoKind::Ddl => {
            println!("{}", db::table_ddl(client, &schema, &table).await?);
        }
        TableInfoKind::Columns => {
            let sql = db::columns_sql(&db::regclass_literal(&schema, &table));
            let rows = db::query_rows(client, &sql).await?;
            output::print_rows(
                &["name", "type", "nullable", "pk", "default", "comment"],
                &rows,
                a.json,
            );
        }
        TableInfoKind::Indexes => {
            let sql = db::indexes_sql(&db::regclass_literal(&schema, &table));
            let rows = db::query_rows(client, &sql).await?;
            output::print_rows(
                &["name", "unique", "primary", "columns", "definition"],
                &rows,
                a.json,
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

// --- profiles -------------------------------------------------------------------

fn cmd_profiles(cmd: ProfilesCmd) -> Result<ExitCode, AppError> {
    match cmd {
        ProfilesCmd::List { json } => {
            let profiles = store::load_profiles()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&profiles).unwrap());
                return Ok(ExitCode::SUCCESS);
            }
            let rows: Vec<Vec<Option<String>>> = profiles
                .iter()
                .map(|p| {
                    vec![
                        Some(p.name.clone()),
                        p.group.clone(),
                        Some(format!("{}:{}", p.host, p.port)),
                        Some(p.database.clone()),
                        Some(p.user.clone()),
                        p.ssh.as_ref().map(|s| s.host.clone()),
                        Some(if p.has_password { "yes" } else { "no" }.into()),
                    ]
                })
                .collect();
            output::print_rows(
                &["name", "group", "host", "db", "user", "ssh", "pw"],
                &rows,
                false,
            );
        }
        ProfilesCmd::Show { alias } => {
            let p = session::resolve_profile(&alias)?;
            println!("{}", serde_json::to_string_pretty(&p).unwrap());
        }
        ProfilesCmd::Rm { alias, yes } => {
            let p = session::resolve_profile(&alias)?;
            if !yes {
                if !std::io::stdin().is_terminal() {
                    return Err(AppError::Msg(
                        "нет TTY для подтверждения — добавь --yes".into(),
                    ));
                }
                eprint!("удалить профиль '{}' ({}:{}/{})? [y/N] ", p.name, p.host, p.port, p.database);
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if !matches!(line.trim(), "y" | "Y" | "yes") {
                    eprintln!("отменено");
                    return Ok(ExitCode::FAILURE);
                }
            }
            store::delete_profile(&p.id)?;
            println!("профиль '{}' удалён", p.name);
        }
    }
    Ok(ExitCode::SUCCESS)
}

// --- history ----------------------------------------------------------------------

fn cmd_history(a: HistoryArgs) -> Result<ExitCode, AppError> {
    if a.scan {
        return history_scan();
    }
    let mut entries = store::load_history()?;
    if let Some(alias) = &a.alias {
        let al = alias.to_lowercase();
        entries.retain(|h| h.profile_name.to_lowercase() == al || h.profile_id == *alias);
    }
    entries.truncate(a.limit);
    if a.json {
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
        return Ok(ExitCode::SUCCESS);
    }
    let now = session::now_ms();
    let rows: Vec<Vec<Option<String>>> = entries
        .iter()
        .map(|h| {
            vec![
                Some(age(now - h.at)),
                Some(h.profile_name.clone()),
                Some(if h.ok { "ok" } else { "err" }.into()),
                Some(one_line(&h.sql, 100)),
            ]
        })
        .collect();
    output::print_rows(&["when", "profile", "st", "sql"], &rows, false);
    Ok(ExitCode::SUCCESS)
}

/// Прогоняет history.json через `sec scan` — не осел ли где секрет в открытом
/// виде (kai редактирует пароли при записи, но старые записи или неожиданные
/// литералы мог поймать sec).
fn history_scan() -> Result<ExitCode, AppError> {
    sec::available()?;
    let path = sql_tauri_lib::fsio::config_path("history.json")?;
    if !path.exists() {
        println!("history.json пуст — сканировать нечего");
        return Ok(ExitCode::SUCCESS);
    }
    let (found, report) = sec::scan(&path.to_string_lossy())?;
    if found {
        println!("{report}");
        println!("⚠ в истории найдены значения секретов из sec — почисти: `sec forget <ключ>` и удали затронутые записи в history.json");
        Ok(ExitCode::FAILURE)
    } else {
        println!("чисто: секретов sec в history.json не найдено");
        Ok(ExitCode::SUCCESS)
    }
}

fn age(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

fn one_line(sql: &str, max: usize) -> String {
    let flat = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

// --- saved ------------------------------------------------------------------------

/// Ключ коллекции сохранёнок для профиля: группа, иначе id.
fn saved_scope(p: &Profile) -> String {
    p.group.clone().filter(|g| !g.trim().is_empty()).unwrap_or_else(|| p.id.clone())
}

async fn cmd_saved(cmd: SavedCmd) -> Result<ExitCode, AppError> {
    match cmd {
        SavedCmd::List { alias, json } => {
            let mut queries = store::load_queries()?;
            if let Some(alias) = &alias {
                let key = saved_scope(&session::resolve_profile(alias)?);
                queries.retain(|q| q.scope.is_none() || q.scope.as_deref() == Some(&key));
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&queries).unwrap());
                return Ok(ExitCode::SUCCESS);
            }
            let rows: Vec<Vec<Option<String>>> = queries
                .iter()
                .map(|q| {
                    vec![
                        Some(q.name.clone()),
                        Some(q.scope.clone().unwrap_or_else(|| "(global)".into())),
                        Some(one_line(&q.sql, 80)),
                    ]
                })
                .collect();
            output::print_rows(&["name", "scope", "sql"], &rows, false);
            Ok(ExitCode::SUCCESS)
        }
        SavedCmd::Run {
            alias,
            name,
            fmt,
            max_rows,
            write,
            password_env,
            from_sec,
            sec_key,
            no_redact,
            no_mux,
            verbose,
        } => {
            let profile = session::resolve_profile(&alias)?;
            let key = saved_scope(&profile);
            let queries = store::load_queries()?;
            let query = queries
                .iter()
                .filter(|q| q.scope.is_none() || q.scope.as_deref() == Some(&key))
                .find(|q| q.name.eq_ignore_ascii_case(&name))
                .ok_or_else(|| {
                    AppError::Msg(format!(
                        "сохранённый запрос '{name}' не найден (см. `kai saved list {alias}`)"
                    ))
                })?;
            cmd_query(QueryArgs {
                alias,
                commands: vec![query.sql.clone()],
                files: vec![],
                fmt,
                max_rows,
                write,
                password_env,
                from_sec,
                sec_key,
                no_history: false,
                no_redact,
                no_mux,
                local: false,
                verbose,
            })
            .await
        }
    }
}

// --- tunnel -----------------------------------------------------------------------

fn cmd_tunnel(cmd: TunnelCmd) -> Result<ExitCode, AppError> {
    match cmd {
        TunnelCmd::List => {
            let masters = tunnel::list_masters();
            if masters.is_empty() {
                println!("нет живых ssh-мастеров");
                return Ok(ExitCode::SUCCESS);
            }
            let rows: Vec<Vec<Option<String>>> = masters
                .iter()
                .map(|m| {
                    vec![
                        Some(m.target.clone()),
                        Some(if m.alive { "alive" } else { "stale" }.into()),
                    ]
                })
                .collect();
            output::print_rows(&["target", "state"], &rows, false);
        }
        TunnelCmd::Close { target, all } => {
            let only = if all { None } else { target.as_deref() };
            if only.is_none() && !all {
                return Err(AppError::Msg(
                    "укажи хост/имя сокета или --all".into(),
                ));
            }
            let n = tunnel::close_masters(only);
            println!("закрыто мастеров: {n}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

// --- vault ------------------------------------------------------------------------

fn cmd_vault(cmd: VaultCmd) -> Result<ExitCode, AppError> {
    match cmd {
        VaultCmd::Status => {
            println!("vault:     {}", if vault::exists() { "есть" } else { "нет" });
            println!(
                "touch id:  {}",
                if vault::biometric_enrolled() { "включён" } else { "выключен" }
            );
            println!(
                "cli trust: {}",
                if vault::cli_trust_enrolled() { "включён" } else { "выключен" }
            );
        }
        VaultCmd::Setup => {
            if vault::exists() {
                return Err(AppError::Msg("vault уже существует".into()));
            }
            let pw = session::read_new_password()?;
            vault::setup(&pw)?;
            println!("vault создан и разблокирован; пароли профилей будут храниться в нём");
        }
        VaultCmd::Trust => {
            session::unlock_vault()?;
            vault::enable_cli_trust()?;
            println!(
                "cli trust включён: kai будет читать пароли без запроса \
                 (копия ключа в login keychain; отозвать — `kai vault revoke`)"
            );
        }
        VaultCmd::Revoke => {
            vault::disable_cli_trust();
            println!("cli trust отозван");
        }
    }
    Ok(ExitCode::SUCCESS)
}
