//! `kai saved` — сохранённые запросы (общие с GUI): список видимых профилю и
//! запуск по имени.

use std::process::ExitCode;

use clap::Subcommand;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, Profile};

use crate::cmd::history::one_line;
use crate::cmd::query::{self, FormatArgs, QueryArgs};
use crate::{output, session};

#[derive(Subcommand)]
pub enum SavedCmd {
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

/// Ключ коллекции сохранёнок для профиля: группа, иначе id.
fn saved_scope(p: &Profile) -> String {
    p.group.clone().filter(|g| !g.trim().is_empty()).unwrap_or_else(|| p.id.clone())
}

pub async fn run(cmd: SavedCmd) -> Result<ExitCode, AppError> {
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
            query::run(QueryArgs {
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
