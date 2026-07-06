//! kai — CLI к sql-kai: выполняет SQL в базах из профилей приложения (общие
//! profiles.json / vault / история с GUI), заводит профили дискавери по ssh
//! (`kai discover`) и умеет прямой fallback через ssh+docker exec (`kai exec`).
//!
//! Сессия по умолчанию read-only (`SET default_transaction_read_only = on`);
//! запись — только с явным `--write`.

mod discover;
mod execmode;
mod output;
mod remote;
mod session;

use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use sql_tauri_lib::db;
use sql_tauri_lib::error::AppError;
use sql_tauri_lib::store::{self, HistoryEntry, Profile};
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
        kai tables orchestrator\n  \
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
    /// Vault с паролями БД
    Vault {
        #[command(subcommand)]
        cmd: VaultCmd,
    },
}

#[derive(Args)]
struct FormatArgs {
    /// JSON-вывод (columns/rows/rowsAffected/truncated)
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
    /// Не записывать запрос в историю
    #[arg(long)]
    no_history: bool,
    /// Показать, куда подключились
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Args)]
struct DiscoverArgs {
    /// SSH-алиас хоста (из ~/.ssh/config*)
    alias: String,
    /// Имя профиля (по умолчанию = alias)
    #[arg(long)]
    name: Option<String>,
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
        #[arg(short, long)]
        verbose: bool,
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
    const KNOWN: &[&str] = &[
        "q", "query", "exec", "discover", "profiles", "tables", "columns", "ddl", "indexes",
        "history", "saved", "vault", "help",
    ];
    let mut args: Vec<OsString> = std::env::args_os().collect();
    if let Some(first) = args.get(1) {
        let s = first.to_string_lossy();
        if !s.starts_with('-') && !KNOWN.contains(&s.as_ref()) {
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
        Cmd::Vault { cmd } => cmd_vault(cmd),
    }
}

// --- q ------------------------------------------------------------------------

async fn cmd_query(a: QueryArgs) -> Result<ExitCode, AppError> {
    let sql = session::collect_sql(&a.commands, &a.files)?;
    let (profile, connected) =
        session::open_for(&a.alias, a.password_env.as_deref(), a.write, a.verbose).await?;

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
            output::print_exec(&exec, a.fmt.pick());
            if a.verbose {
                eprintln!("({} ms)", exec.duration_ms);
            }
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

// --- introspection --------------------------------------------------------------

async fn cmd_tables(a: TablesArgs) -> Result<ExitCode, AppError> {
    let (_, connected) =
        session::open_for(&a.alias, a.password_env.as_deref(), false, a.verbose).await?;
    let rows = db::query_rows(&connected.session.client, db::TABLES_SQL).await?;
    let mapped: Vec<Vec<Option<String>>> = rows
        .into_iter()
        .map(|row| {
            let kind = match row.get(2).and_then(|v| v.as_deref()) {
                Some("v") => "view",
                Some("m") => "matview",
                Some("f") => "foreign",
                _ => "table",
            };
            vec![row[0].clone(), row[1].clone(), Some(kind.to_string())]
        })
        .collect();
    output::print_rows(&["schema", "name", "kind"], &mapped, a.json);
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
    let (_, connected) =
        session::open_for(&a.alias, a.password_env.as_deref(), false, a.verbose).await?;
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
                no_history: false,
                verbose,
            })
            .await
        }
    }
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
