//! kai — CLI к sql-kai: выполняет SQL в базах из профилей приложения (общие
//! profiles.json / vault / история с GUI), заводит профили дискавери по ssh
//! (`kai discover`) и умеет прямой fallback через ssh+docker exec (`kai exec`).
//!
//! Сессия по умолчанию read-only (`SET default_transaction_read_only = on`);
//! запись — только с явным `--write`. Чувствительные колонки (password/
//! secret/*_token/*_key) в выводе маскируются; отключение — `--no-redact`.

mod broker_client;
mod cmd;
mod input;
mod output;
mod redact;
mod remote;
mod sec;
mod session;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use sql_kai_lib::error::AppError;

use cmd::discover::DiscoverArgs;
use cmd::doctor::DoctorArgs;
use cmd::exec::ExecArgs;
use cmd::history::HistoryArgs;
use cmd::introspect::{TableArgs, TableInfoKind, TablesArgs};
use cmd::profiles::ProfilesCmd;
use cmd::query::QueryArgs;
use cmd::rotate::RotateArgs;
use cmd::saved::SavedCmd;
use cmd::sessions::SessionsArgs;
use cmd::tunnel::TunnelCmd;
use cmd::vault::VaultCmd;

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
    #[command(name = "q", alias = "query")]
    Query(QueryArgs),
    /// Прямой режим без профиля: ssh <alias> -> docker exec psql
    Exec(ExecArgs),
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
        Cmd::Query(a) => cmd::query::run(a).await,
        Cmd::Exec(a) => cmd::exec::run(a),
        Cmd::Discover(a) => cmd::discover::run(a).await,
        Cmd::Profiles { cmd } => cmd::profiles::run(cmd.unwrap_or(ProfilesCmd::List { fmt: Default::default() })).await,
        Cmd::Tables(a) => cmd::introspect::tables(a).await,
        Cmd::Columns(a) => cmd::introspect::table_info(a, TableInfoKind::Columns).await,
        Cmd::Ddl(a) => cmd::introspect::table_info(a, TableInfoKind::Ddl).await,
        Cmd::Indexes(a) => cmd::introspect::table_info(a, TableInfoKind::Indexes).await,
        Cmd::History(a) => cmd::history::run(a),
        Cmd::Saved { cmd } => cmd::saved::run(cmd).await,
        Cmd::Rotate(a) => cmd::rotate::run(a).await,
        Cmd::Doctor(a) => cmd::doctor::run(a).await,
        Cmd::Sessions(a) => cmd::sessions::run(a).await,
        Cmd::Tunnel { cmd } => cmd::tunnel::run(cmd.unwrap_or(TunnelCmd::List)),
        Cmd::Vault { cmd } => cmd::vault::run(cmd),
    }
}
