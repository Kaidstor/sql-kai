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
use cmd::feedback::FeedbackArgs;
use cmd::fork::ForkArgs;
use cmd::history::HistoryArgs;
use cmd::holder::HolderCmd;
use cmd::import::ImportArgs;
use cmd::init::InitArgs;
use cmd::table_info::{TableArgs, TableInfoKind};
use cmd::tables::TablesArgs;
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
    /// Импорт профилей: --from beekeeper|dbeaver или JSON (stdin/файл);
    /// пароли — в vault
    Import(ImportArgs),
    /// Копия базы профиля в локальном docker + профиль на неё (миграции — не на проде)
    Fork(ForkArgs),
    /// Профили (общие с GUI)
    Profiles {
        #[command(subcommand)]
        cmd: Option<ProfilesCmd>,
    },
    /// Схема базы одним дампом: таблицы, вьюхи, enum, функции (--table — одна таблица)
    Schema(SchemaArgs),
    /// Список таблиц/вьюх базы
    Tables(TablesArgs),
    /// Колонки таблицы (структура целиком — schema --table)
    Columns(TableArgs),
    /// DDL таблицы (CREATE TABLE / VIEW)
    Ddl(TableArgs),
    /// Индексы таблицы (структура целиком — schema --table)
    Indexes(TableArgs),
    /// MCP-сервер (stdio) для AI-агентов: tools query/schema/tables/ddl +
    /// open_table/open_query (вкладки в GUI); mcp install — прописать сервер
    /// в конфиг MCP-клиента, mcp status — где он уже прописан
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
    /// Ссылка на issue с диагностикой (ничего не отправляет сам)
    Feedback(FeedbackArgs),
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

/// Имена подкоманд вместе с алиасами — прямо из определения clap.
/// Single source of truth: новый вариант `Cmd` не может молча начать читаться
/// как алиас профиля.
fn subcommand_names() -> Vec<String> {
    use clap::CommandFactory;
    Cli::command()
        .get_subcommands()
        .flat_map(|c| {
            std::iter::once(c.get_name().to_string())
                .chain(c.get_all_aliases().map(str::to_string))
        })
        .chain(std::iter::once("help".to_string()))
        .collect()
}

/// `sql-kai <alias> -c ...` — шорткат для `sql-kai q <alias> -c ...`: если первый
/// аргумент не подкоманда и не флаг, считаем его алиасом профиля.
fn preprocess(mut args: Vec<OsString>, known: &[String]) -> Vec<OsString> {
    if let Some(first) = args.get(1) {
        let s = first.to_string_lossy();
        if !s.starts_with('-') && !known.iter().any(|k| k == s.as_ref()) {
            args.insert(1, "q".into());
        }
    }
    args
}

/// Профиль, чьё имя совпало с именем подкоманды, шорткатом `sql-kai <alias>`
/// недоступен: первым аргументом его всегда перехватит подкоманда. Молча это
/// выглядит как «после обновления sql-kai schema сломался», поэтому в момент,
/// когда подкоманда не разобрала аргументы, называем обходной путь. Проверка
/// профилей — замыканием: на успешном разборе в profiles.json не ходим вовсе.
fn shortcut_hint(first: &str, profile_exists: impl FnOnce(&str) -> bool) -> Option<String> {
    // `q` первым аргументом — это либо явный вызов, либо уже развёрнутый нами
    // шорткат: коллизии имён там нет по определению.
    if first.starts_with('-') || first == "q" || !profile_exists(first) {
        return None;
    }
    Some(format!(
        "подсказка: `{first}` — подкоманда sql-kai, поэтому первым аргументом она \
         заслоняет одноимённый профиль.\n  запрос к профилю: sql-kai q {first} -c \"…\""
    ))
}

/// clap печатает ошибку разбора (и --help/--version) сам; мы дописываем к ней
/// подсказку про заслонённый профиль и повторяем его код выхода.
fn report_parse_error(err: clap::Error, args: &[OsString]) -> ExitCode {
    let _ = err.print();
    if err.use_stderr() {
        let hint = args.get(1).and_then(|first| {
            shortcut_hint(&first.to_string_lossy(), |alias| {
                let profiles = sql_kai_lib::store::load_profiles().unwrap_or_default();
                !session::filter_profiles(&profiles, alias).is_empty()
            })
        });
        if let Some(hint) = hint {
            eprintln!("\n{hint}");
        }
    }
    ExitCode::from(err.exit_code().clamp(0, 255) as u8)
}

fn main() -> ExitCode {
    let args = preprocess(std::env::args_os().collect(), &subcommand_names());
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) => return report_parse_error(e, &args),
    };
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
        Cmd::Profiles { cmd } => {
            cmd::profiles::run(cmd.unwrap_or_else(ProfilesCmd::default_list)).await
        }
        Cmd::Schema(a) => cmd::schema::run(a).await,
        Cmd::Tables(a) => cmd::tables::run(a).await,
        Cmd::Columns(a) => cmd::table_info::run(a, TableInfoKind::Columns).await,
        Cmd::Ddl(a) => cmd::table_info::run(a, TableInfoKind::Ddl).await,
        Cmd::Indexes(a) => cmd::table_info::run(a, TableInfoKind::Indexes).await,
        Cmd::Mcp(a) => cmd::mcp::run(a).await,
        Cmd::History(a) => cmd::history::run(a),
        Cmd::Saved { cmd } => cmd::saved::run(cmd).await,
        Cmd::Rotate(a) => cmd::rotate::run(a).await,
        Cmd::Doctor(a) => cmd::doctor::run(a).await,
        Cmd::Feedback(a) => cmd::feedback::run(a).await,
        Cmd::Sessions(a) => cmd::sessions::run(a).await,
        Cmd::Tunnel { cmd } => cmd::tunnel::run(cmd.unwrap_or(TunnelCmd::List)),
        Cmd::Vault { cmd } => cmd::vault::run(cmd).await,
        Cmd::Holder { cmd } => cmd::holder::run(cmd).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    /// Шорткат профиля разворачивается только для слова, которого нет среди
    /// подкоманд и их алиасов.
    #[test]
    fn shortcut_expands_only_an_unknown_first_word() {
        let known = subcommand_names();
        assert_eq!(
            preprocess(args(&["sql-kai", "domainator", "-c", "SELECT 1"]), &known),
            args(&["sql-kai", "q", "domainator", "-c", "SELECT 1"])
        );
        // подкоманда, её алиас, help и флаг остаются как есть
        for first in ["schema", "logs", "query", "help", "--version"] {
            let given = args(&["sql-kai", first, "x"]);
            assert_eq!(preprocess(given.clone(), &known), given, "{first}");
        }
        // без аргументов вообще: usage напечатает clap
        assert_eq!(preprocess(args(&["sql-kai"]), &known), args(&["sql-kai"]));
    }

    /// Профиль, названный как подкоманда, шорткатом недоступен — обход должен
    /// быть назван, иначе смена поведения выглядит как поломка `sql-kai schema`.
    #[test]
    fn hint_names_the_explicit_form_for_a_shadowed_profile() {
        let hint = shortcut_hint("schema", |_| true).expect("подсказка");
        assert!(hint.contains("sql-kai q schema"), "{hint}");
        // профиля с таким именем нет — подсказка была бы мусором
        assert!(shortcut_hint("schema", |_| false).is_none());
        // явный `q` и флаги коллизией не являются
        assert!(shortcut_hint("q", |_| true).is_none());
        assert!(shortcut_hint("--json", |_| true).is_none());
    }
}
