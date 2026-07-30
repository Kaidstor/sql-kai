//! `sql-kai tables` — список таблиц и вьюх профиля.

use std::process::ExitCode;

use clap::Args;
use sql_kai_lib::db;
use sql_kai_lib::error::AppError;

use crate::output::{self, FormatArgs};
use crate::session;

#[derive(Args)]
pub struct TablesArgs {
    /// Профиль: имя, id или группа
    alias: String,
    /// Примерное число строк (pg_class.reltuples; '?' = не анализировалась)
    #[arg(long)]
    counts: bool,
    #[command(flatten)]
    fmt: FormatArgs,
    #[arg(long, value_name = "VAR")]
    password_env: Option<String>,
    #[arg(short, long)]
    verbose: bool,
}

pub async fn run(a: TablesArgs) -> Result<ExitCode, AppError> {
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
    output::print_rows(headers, &mapped, a.fmt.pick());
    Ok(ExitCode::SUCCESS)
}
