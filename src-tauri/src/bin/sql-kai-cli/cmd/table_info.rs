//! `sql-kai columns/ddl/indexes` — интроспекция одной таблицы: колонки, DDL,
//! индексы. Три подкоманды делят один модуль: аргументы и открытие сессии у
//! них общие, различается только запрос (см. [`TableInfoKind`]).

use std::process::ExitCode;

use clap::Args;
use sql_kai_lib::db;
use sql_kai_lib::error::AppError;

use crate::output::{self, FormatArgs};
use crate::session;

#[derive(Args)]
pub struct TableArgs {
    /// Профиль: имя, id или группа
    alias: String,
    /// Таблица: [schema.]table (по умолчанию схема public)
    table: String,
    #[command(flatten)]
    fmt: FormatArgs,
    #[arg(long, value_name = "VAR")]
    password_env: Option<String>,
    #[arg(short, long)]
    verbose: bool,
}

pub enum TableInfoKind {
    Columns,
    Ddl,
    Indexes,
}

/// "schema.table" -> (schema, table); без точки — public.
pub(crate) fn split_table(spec: &str) -> (String, String) {
    match spec.split_once('.') {
        Some((s, t)) => (s.to_string(), t.to_string()),
        None => ("public".to_string(), spec.to_string()),
    }
}

pub async fn run(a: TableArgs, kind: TableInfoKind) -> Result<ExitCode, AppError> {
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
                a.fmt.pick(),
            );
        }
        TableInfoKind::Indexes => {
            let sql = db::indexes_sql(&db::regclass_literal(&schema, &table));
            let rows = db::query_rows(client, &sql).await?;
            output::print_rows(
                &["name", "unique", "primary", "columns", "definition"],
                &rows,
                a.fmt.pick(),
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
