use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use serde::Serialize;
use tokio_postgres::types::Type;
use tokio_postgres::{Client, SimpleQueryMessage};

use super::sqltext::{
    advance_tx, escapes_read_only_tx, reaches_server_side_io, split_statements, TxStatus,
};
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
                    cur.rows.push(
                        (0..row.len())
                            .map(|i| row.get(i).map(str::to_string))
                            .collect(),
                    );
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

/// Runs `sql` inside an explicit `BEGIN READ ONLY` block — the strongest
/// read-only guarantee available on a session whose role we do not own.
///
/// The session-wide `SET default_transaction_read_only = on` this used to rely
/// on is a USERSET GUC: any batch could switch it off and write. Inside a
/// read-only transaction Postgres refuses writes *and* `SET TRANSACTION READ
/// WRITE` (SQLSTATE 25001), so the only way out is to end the block — which is
/// what [`escapes_read_only_tx`] refuses up front.
///
/// What the block covers is *database* writes, and nothing wider: `COPY … TO
/// PROGRAM`, `COPY … TO '/path'` and `lo_export` touch the server's shell and
/// filesystem without ever setting the flag Postgres checks, so they need their
/// own gate ([`reaches_server_side_io`]).
///
/// `BEGIN`/`COMMIT` are sent as their own statements on purpose: folding them
/// into the batch string would add their `CommandComplete` to [`ExecResult`]
/// and break the 1:1 statement-to-result mapping that the CLI renderer, the MCP
/// structured output and `sql-kai schema`'s result-set parsing all rely on.
pub async fn execute_read_only(
    client: &Client,
    sql: &str,
    max_rows: usize,
) -> Result<ExecResult, AppError> {
    begin_read_only(client, sql).await?;
    let result = execute(client, sql, max_rows).await;
    let closed = end_read_only(client, result.is_ok()).await;
    match (result, closed) {
        (Ok(exec), Ok(_)) => Ok(exec),
        // The read succeeded but the block is still open — the caller has to
        // know, otherwise the next call inherits a transaction it never opened.
        (Ok(_), Err(e)) => Err(e),
        (Err(e), _) => Err(e),
    }
}

/// Gate checks + `BEGIN READ ONLY` — the opening half of
/// [`execute_read_only`], separate so the export path (which streams rows
/// instead of collecting an [`ExecResult`]) runs `sql` under the same block.
/// Every `Ok(())` MUST be paired with [`end_read_only`].
pub async fn begin_read_only(client: &Client, sql: &str) -> Result<(), AppError> {
    if escapes_read_only_tx(sql) {
        return Err(AppError::ReadOnlyRefused(
            "read-only session: the batch would leave the read-only transaction it \
             runs in, or lift its read-only mode (COMMIT/ROLLBACK/END/ABORT/PREPARE \
             TRANSACTION/DISCARD/SET TRANSACTION/SET …transaction_read_only, including \
             its set_config() spelling). Re-run with write access enabled if it is \
             meant to modify data."
                .into(),
        ));
    }
    if reaches_server_side_io(sql) {
        return Err(AppError::ReadOnlyRefused(
            "read-only session: the batch would write outside the database — COPY … TO \
             PROGRAM runs a shell on the server, COPY … TO '/path' and lo_export write \
             its filesystem. A read-only transaction does not cover any of these. Use \
             COPY … TO STDOUT to stream data back instead."
                .into(),
        ));
    }
    // `SELECT 1` здесь не косметика: Postgres разрешает `SET TRANSACTION READ
    // WRITE` в read-only транзакции, пока та не взяла снапшот
    // (check_transaction_read_only смотрит на FirstSnapshotSet), а голый BEGIN
    // его не берёт. Один запрос закрывает это окно, и повышение прав изнутри
    // блока падает с 25001. Результат вызова отбрасывается, на ExecResult
    // батча он не влияет.
    execute(client, "BEGIN READ ONLY; SELECT 1", 1).await?;
    Ok(())
}

/// Closes the read-only block either way: after a failed statement the
/// transaction is aborted and every later statement on this connection errors
/// until it is rolled back, so leaving it open would poison a pooled session.
pub async fn end_read_only(client: &Client, ok: bool) -> Result<(), AppError> {
    execute(client, if ok { "COMMIT" } else { "ROLLBACK" }, 1)
        .await
        .map(|_| ())
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

    /// [`execute_read_only`] + status tracking. The status is set to `Idle`
    /// rather than folded from the SQL: the wrapper always closes its block, so
    /// the connection is out of any transaction whatever the batch did — a
    /// user-typed `BEGIN` inside it is the no-op warning Postgres makes it, not
    /// a transaction left open. If even the closing statement failed the
    /// connection is gone, and the next call surfaces that.
    pub async fn execute_read_only(
        &self,
        sql: &str,
        max_rows: usize,
    ) -> Result<ExecResult, AppError> {
        let result = execute_read_only(self.client, sql, max_rows).await;
        self.mark_idle();
        result
    }

    /// Форсирует Idle — для путей, которые открыли и закрыли read-only блок
    /// сами ([`begin_read_only`]/[`end_read_only`], export) и потому знают,
    /// что соединение вне транзакции, что бы ни было в батче.
    pub fn mark_idle(&self) {
        self.tx.store(TxStatus::Idle as u8, Ordering::Relaxed);
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
pub async fn query_rows(client: &Client, sql: &str) -> Result<Vec<Vec<Option<String>>>, AppError> {
    let exec = execute(client, sql, INTROSPECT_MAX_ROWS).await?;
    Ok(exec.results.into_iter().flat_map(|r| r.rows).collect())
}

/// First column of the first row — for single-value catalog lookups.
pub async fn query_scalar(client: &Client, sql: &str) -> Result<Option<String>, AppError> {
    Ok(query_rows(client, sql)
        .await?
        .first()
        .and_then(|r| r[0].clone()))
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
