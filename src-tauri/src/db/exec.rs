use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use serde::Serialize;
use tokio_postgres::types::Type;
use tokio_postgres::{Client, SimpleQueryMessage};

use super::sqltext::{advance_tx, split_statements, TxStatus};
use crate::error::AppError;

#[derive(Serialize, serde::Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatementResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub rows_affected: Option<u64>,
    pub truncated: bool,
}

impl StatementResult {
    /// Empty result carrying just the column names.
    fn with_columns(columns: Vec<String>) -> Self {
        StatementResult {
            columns,
            ..Default::default()
        }
    }
}

#[derive(Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub results: Vec<StatementResult>,
    pub duration_ms: u64,
}

/// Row cap for catalog introspection ([`query_rows`]) — a runaway guard rather
/// than a display limit, hence far above the human-facing defaults.
pub const INTROSPECT_MAX_ROWS: usize = 10_000;

/// Runs SQL through the simple-query protocol: multiple `;`-separated statements
/// are supported and every value arrives already text-formatted by the server.
///
/// `max_rows` is a per-statement display cap, deliberately different per caller
/// — every value below is a default the caller can override, not a hard limit:
/// - **1000** — GUI (`commands::execute_sql`), CLI `q` (`--max-rows`) and the
///   broker (`default_max_rows`): what a human scrolls through in one result grid.
/// - **200** — MCP (`cmd::mcp::DEFAULT_MAX_ROWS`, `maxRows` in the tool schema):
///   rows land in an LLM context window, so the default is deliberately tighter.
/// - **[`INTROSPECT_MAX_ROWS`]** — catalog introspection, see above.
///
/// GUI and broker additionally clamp the requested value to `1..=100_000`.
pub async fn execute(client: &Client, sql: &str, max_rows: usize) -> Result<ExecResult, AppError> {
    let start = Instant::now();
    let messages = client.simple_query(sql).await?;
    let mut results: Vec<StatementResult> = Vec::new();
    let mut current: Option<StatementResult> = None;

    for msg in messages {
        match msg {
            SimpleQueryMessage::RowDescription(cols) => {
                current = Some(StatementResult::with_columns(
                    cols.iter().map(|c| c.name().to_string()).collect(),
                ));
            }
            SimpleQueryMessage::Row(row) => {
                let cur = current.get_or_insert_with(|| {
                    StatementResult::with_columns(
                        row.columns().iter().map(|c| c.name().to_string()).collect(),
                    )
                });
                if cur.rows.len() < max_rows {
                    cur.rows
                        .push((0..row.len()).map(|i| row.get(i).map(str::to_string)).collect());
                } else {
                    cur.truncated = true;
                }
            }
            SimpleQueryMessage::CommandComplete(n) => {
                let mut res = current.take().unwrap_or_default();
                res.rows_affected = Some(n);
                results.push(res);
            }
            _ => {}
        }
    }
    if let Some(res) = current.take() {
        results.push(res);
    }

    Ok(ExecResult {
        results,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Runs SQL on a session while keeping its heuristic transaction status
/// ([`Session::tx`](super::Session)) in sync: load → execute → advance_tx →
/// store. The single query path shared by the GUI commands and the broker,
/// which used to carry a copy of this bookkeeping each.
pub struct QueryExecutor<'a> {
    client: &'a Client,
    tx: &'a AtomicU8,
}

impl<'a> QueryExecutor<'a> {
    pub fn new(client: &'a Client, tx: &'a AtomicU8) -> Self {
        QueryExecutor { client, tx }
    }

    /// Current (heuristic) transaction status of the session.
    pub fn status(&self) -> TxStatus {
        TxStatus::from_u8(self.tx.load(Ordering::Relaxed))
    }

    /// Advances the tracked status after `sql` ran with outcome `ok`. Public
    /// separately from [`Self::execute`] for callers that run their SQL some
    /// other way (export streams rows to a file) but must track it the same.
    pub fn advance(&self, before: TxStatus, sql: &str, ok: bool) {
        self.tx
            .store(advance_tx(before, sql, ok) as u8, Ordering::Relaxed);
    }

    /// [`execute`] + status tracking, on success or failure both — a failed
    /// statement inside a tx leaves it aborted, which the status surfaces.
    pub async fn execute(&self, sql: &str, max_rows: usize) -> Result<ExecResult, AppError> {
        let before = self.status();
        let result = execute(self.client, sql, max_rows).await;
        self.advance(before, sql, result.is_ok());
        result
    }
}

/// Cell text at `i`; "" for NULL or a missing column.
pub fn cell(row: &[Option<String>], i: usize) -> String {
    row.get(i).cloned().flatten().unwrap_or_default()
}

/// Boolean cell rendered by Postgres as "true"/"false".
pub fn cell_bool(row: &[Option<String>], i: usize) -> bool {
    row.get(i).and_then(|v| v.as_deref()) == Some("true")
}

/// All rows of every result — for catalog queries. The 10k cap is a runaway
/// guard, not a display limit (see [`execute`] on why the defaults differ).
pub async fn query_rows(
    client: &Client,
    sql: &str,
) -> Result<Vec<Vec<Option<String>>>, AppError> {
    let exec = execute(client, sql, INTROSPECT_MAX_ROWS).await?;
    Ok(exec.results.into_iter().flat_map(|r| r.rows).collect())
}

/// First column of the first row — for single-value catalog lookups.
pub async fn query_scalar(client: &Client, sql: &str) -> Result<Option<String>, AppError> {
    Ok(query_rows(client, sql).await?.first().and_then(|r| r[0].clone()))
}

/// Column (name, type) per statement, obtained by preparing each one (Parse
/// only — nothing is executed). None for statements that fail to prepare,
/// e.g. ones referencing objects created earlier in the same batch.
pub async fn statement_column_types(
    client: &Client,
    sql: &str,
) -> Vec<Option<Vec<(String, Type)>>> {
    let mut out = Vec::new();
    for stmt in split_statements(sql) {
        let cols = client.prepare(&stmt).await.ok().map(|s| {
            s.columns()
                .iter()
                .map(|c| (c.name().to_string(), c.type_().clone()))
                .collect()
        });
        out.push(cols);
    }
    out
}
