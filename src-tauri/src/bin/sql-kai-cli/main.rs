//! sql-kai — CLI к sql-kai: выполняет SQL в базах из профилей приложения (общие
//! profiles.json / vault / история с GUI), заводит профили дискавери по ssh
//! (`sql-kai discover`) и умеет прямой fallback через ssh+docker exec (`sql-kai exec`).
//!
//! Сессия по умолчанию read-only (`SET default_transaction_read_only = on`);
//! запись — только с явным `--write`. Чувствительные колонки (password/
//! secret/*_token/*_key) в выводе маскируются; отключение — `--no-redact`.

mod broker_client;
mod cmd;
mod envvar;
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

use cmd::completion::CompletionArgs;
use cmd::discover::DiscoverArgs;
use cmd::doctor::DoctorArgs;
use cmd::exec::ExecArgs;
use cmd::fork::ForkArgs;
use cmd::history::HistoryArgs;
use cmd::holder::HolderCmd;
use cmd::import::ImportArgs;
use cmd::init::InitArgs;
use cmd::introspect::{TableArgs, TableInfoKind, TablesArgs};
use cmd::logs::LogsArgs;
use cmd::mcp::McpArgs;
use cmd::profiles::ProfilesCmd;
use cmd::query::QueryArgs;
use cmd::rotate::RotateArgs;
use cmd::saved::SavedCmd;
use cmd::schema::SchemaArgs;
use cmd::sessions::SessionsArgs;
use cmd::tunnel::TunnelCmd;
use cmd::vault::VaultCmd;

#[derive(Parser)]
#[command(
    name = "sql-kai",
    // без bin_name usage показывал бы argv[0] — «sql-kai-cli» при прямом
    // запуске sidecar-файла; команда для пользователя всегда sql-kai
    bin_name = "sql-kai",
    version,
    about = "SQL к Postgres по профилям sql-kai (ssh-туннели, vault, история)",
    after_help = "Примеры:\n  \
        sql-kai init                       # первичная настройка: PATH, vault, MCP, автодополнение\n  \
        sql-kai domainator -c \"SELECT count(*) FROM domains\"\n  \
        echo \"SELECT now()\" | sql-kai orchestrator --json\n  \
        sql-kai discover coordinator       # ssh-хост -> профиль (+ пароль в vault)\n  \
        sql-kai exec coordinator -c \"SELECT 1\"   # fallback: ssh + docker exec psql\n  \
        sql-kai tables orchestrator --counts     # таблицы + примерное число строк\n  \
        sql-kai vault trust                # тихий доступ CLI к паролям vault"
)]
// pub(crate), чтобы `sql-kai completion` мог отдать clap-описание генератору
// скриптов — единственный источник правды о подкомандах и флагах.
pub(crate) struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Первичная настройка: sql-kai в PATH, vault trust, MCP, автодополнение
    Init(InitArgs),
    /// Скрипт автодополнения шелла (zsh/bash/fish) + имена профилей
    Completion(CompletionArgs),
    /// Выполнить SQL в базе профиля (sql-kai <alias> — то же самое)
    #[command(name = "q", alias = "query")]
    Query(QueryArgs),
    /// Прямой режим без профиля: ssh <alias> -> docker exec psql
    Exec(ExecArgs),
    /// Журнал postgres-сервера профиля (ssh -> docker logs контейнера)
    Logs(LogsArgs),
    /// Найти postgres на ssh-хосте и создать/обновить профиль
    Discover(DiscoverArgs),
    /// Массовый импорт профилей из JSON (stdin/файл), пароли — в vault
    Import(ImportArgs),
    /// Копия базы профиля в локальном docker + профиль на неё (миграции — не на проде)
    Fork(ForkArgs),
    /// Профили (общие с GUI)
    Profiles {
        #[command(subcommand)]
        cmd: Option<ProfilesCmd>,
    },
    /// Вся схема базы одним дампом: таблицы, вьюхи, enum, функции
    Schema(SchemaArgs),
    /// Список таблиц/вьюх базы
    Tables(TablesArgs),
    /// Колонки таблицы
    Columns(TableArgs),
    /// DDL таблицы (CREATE TABLE / VIEW)
    Ddl(TableArgs),
    /// Индексы таблицы
    Indexes(TableArgs),
    /// MCP-сервер (stdio) для AI-агентов: tools query/tables/columns/ddl/
    /// indexes + open_table/open_query (вкладки в GUI); mcp install — прописать
    /// сервер в конфиг MCP-клиента, mcp status — где он уже прописан
    Mcp(McpArgs),
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
    /// Фоновый держатель cli-сессий (спавнится сам при `sql-kai q`, когда GUI
    /// не запущен) — скрыт: руками нужен разве что `sql-kai holder stop`
    #[command(hide = true)]
    Holder {
        #[command(subcommand)]
        cmd: HolderCmd,
    },
}

/// `sql-kai <alias> -c ...` — шорткат для `sql-kai q <alias> -c ...`: если первый
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
            eprintln!("sql-kai: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn dispatch(cli: Cli) -> Result<ExitCode, AppError> {
    match cli.cmd {
        Cmd::Init(a) => cmd::init::run(a).await,
        Cmd::Completion(a) => cmd::completion::run(a),
        Cmd::Query(a) => cmd::query::run(a).await,
        Cmd::Exec(a) => cmd::exec::run(a),
        Cmd::Logs(a) => cmd::logs::run(a),
        Cmd::Discover(a) => cmd::discover::run(a).await,
        Cmd::Import(a) => cmd::import::run(a).await,
        Cmd::Fork(a) => cmd::fork::run(a).await,
        Cmd::Profiles { cmd } => cmd::profiles::run(cmd.unwrap_or(ProfilesCmd::List { filter: None, fmt: Default::default() })).await,
        Cmd::Schema(a) => cmd::schema::run(a).await,
        Cmd::Tables(a) => cmd::introspect::tables(a).await,
        Cmd::Columns(a) => cmd::introspect::table_info(a, TableInfoKind::Columns).await,
        Cmd::Ddl(a) => cmd::introspect::table_info(a, TableInfoKind::Ddl).await,
        Cmd::Indexes(a) => cmd::introspect::table_info(a, TableInfoKind::Indexes).await,
        Cmd::Mcp(a) => cmd::mcp::run(a).await,
        Cmd::History(a) => cmd::history::run(a),
        Cmd::Saved { cmd } => cmd::saved::run(cmd).await,
        Cmd::Rotate(a) => cmd::rotate::run(a).await,
        Cmd::Doctor(a) => cmd::doctor::run(a).await,
        Cmd::Sessions(a) => cmd::sessions::run(a).await,
        Cmd::Tunnel { cmd } => cmd::tunnel::run(cmd.unwrap_or(TunnelCmd::List)),
        Cmd::Vault { cmd } => cmd::vault::run(cmd).await,
        Cmd::Holder { cmd } => cmd::holder::run(cmd).await,
    }
}
