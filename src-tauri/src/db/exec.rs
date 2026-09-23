use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tokio_postgres::error::SqlState;
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

#[derive(Serialize, serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub results: Vec<StatementResult>,
    pub duration_ms: u64,
    /// Сообщения сервера (`RAISE NOTICE`, WARNING, INFO) за время батча.
    /// Пустые поля не сериализуются: форма ответа для старых потребителей та
    /// же, а старый брокер без них десериализуется в пустое значение.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<Notice>,
    /// Сколько notices не дошло: буфер сессии переполнился (по числу или
    /// байтам) и выбросил самые старые.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub notices_dropped: u64,
    /// Батч оставил транзакцию открытой или прерванной, и её откатили.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_rolled_back: Option<TxLeftover>,
}

/// Асинхронное сообщение сервера — NoticeResponse протокола.
#[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct Notice {
    /// Нелокализованная severity (`NOTICE`, `WARNING`, `INFO`, …).
    pub severity: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Notice {
    pub fn from_db(e: &tokio_postgres::error::DbError) -> Self {
        Notice {
            severity: e
                .parsed_severity()
                .map(|s| s.to_string())
                .unwrap_or_else(|| e.severity().to_string()),
            message: e.message().to_string(),
            detail: e.detail().map(str::to_string),
            hint: e.hint().map(str::to_string),
        }
    }

    /// Байты текста — для бюджетов буфера и ответа.
    pub fn size(&self) -> usize {
        self.severity.len()
            + self.message.len()
            + self.detail.as_deref().map_or(0, str::len)
            + self.hint.as_deref().map_or(0, str::len)
    }

    /// Укорачивает message/detail/hint до `max` байт каждое с пометкой
    /// о срезанном. true — что-то срезано.
    pub fn truncate(&mut self, max: usize) -> bool {
        let mut cut = cut_text(&mut self.message, max);
        for f in [&mut self.detail, &mut self.hint].into_iter().flatten() {
            cut |= cut_text(f, max);
        }
        cut
    }

    /// Строки как у psql: `NOTICE: …`, затем `DETAIL: …` / `HINT: …`.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!("{}: {}", self.severity, self.message)];
        if let Some(d) = &self.detail {
            out.push(format!("DETAIL: {d}"));
        }
        if let Some(h) = &self.hint {
            out.push(format!("HINT: {h}"));
        }
        out
    }
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Режет строку по границе символа и дописывает, сколько байт срезано.
fn cut_text(s: &mut String, max: usize) -> bool {
    if s.len() <= max {
        return false;
    }
    let full = s.len();
    let mut n = max;
    while n > 0 && !s.is_char_boundary(n) {
        n -= 1;
    }
    s.truncate(n);
    s.push_str(&format!("…[{} more bytes cut]", full - n));
    true
}

/// Урезает notices под бюджет ответа: каждое поле — до `per_field` байт,
/// всего — до `total`; не влезшие хвостовые отбрасываются. Возвращает
/// (что-то срезано, сколько отброшено).
pub fn cap_notices(notices: &mut Vec<Notice>, per_field: usize, total: usize) -> (bool, u64) {
    let mut cut = false;
    let mut used = 0usize;
    let mut keep = notices.len();
    for (i, n) in notices.iter_mut().enumerate() {
        cut |= n.truncate(per_field);
        if used + n.size() > total && i > 0 {
            keep = i;
            break;
        }
        used += n.size();
    }
    let dropped = (notices.len() - keep) as u64;
    notices.truncate(keep);
    (cut || dropped > 0, dropped)
}

/// Что накопилось в буфере с прошлого `take`.
#[derive(Debug, Default)]
pub struct NoticeBatch {
    pub notices: Vec<Notice>,
    /// Выброшено при переполнении.
    pub dropped: u64,
}

#[derive(Default)]
struct SinkState {
    q: VecDeque<Notice>,
    bytes: usize,
    dropped: u64,
}

/// Сюда драйвер соединения складывает notices, пока их не заберёт вызывающий.
/// Порядок гарантирован протоколом: NoticeResponse приходит раньше
/// ReadyForQuery своего запроса, а драйвер кладёт его в буфер до того, как
/// отдаст клиенту ответ, — после `simple_query` notices батча уже здесь.
///
/// `NoticeSink::default()` выключен и ничего не копит: вкладки GUI notices
/// не забирают, и включённый буфер там только держал бы память.
#[derive(Clone, Default)]
pub struct NoticeSink(Option<Arc<Mutex<SinkState>>>);

/// Потолки буфера: по числу, по байтам всего и по байтам одного поля.
/// Без байтового предела `RAISE NOTICE '%', repeat('x', 20000000)` в цикле
/// раздувал holder на сотни мегабайт.
const NOTICE_CAP: usize = 1000;
const NOTICE_BUF_BYTES: usize = 1024 * 1024;
const NOTICE_FIELD_BYTES: usize = 64 * 1024;

impl NoticeSink {
    pub fn enabled() -> Self {
        NoticeSink(Some(Arc::default()))
    }

    pub fn push(&self, mut n: Notice) {
        let Some(state) = &self.0 else {
            return;
        };
        n.truncate(NOTICE_FIELD_BYTES);
        let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
        st.bytes += n.size();
        st.q.push_back(n);
        while st.q.len() > NOTICE_CAP || (st.bytes > NOTICE_BUF_BYTES && st.q.len() > 1) {
            if let Some(old) = st.q.pop_front() {
                st.bytes -= old.size();
                st.dropped += 1;
            }
        }
    }

    pub fn take(&self) -> NoticeBatch {
        let Some(state) = &self.0 else {
            return NoticeBatch::default();
        };
        let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
        let batch = NoticeBatch {
            notices: st.q.drain(..).collect(),
            dropped: st.dropped,
        };
        st.bytes = 0;
        st.dropped = 0;
        batch
    }
}

/// В каком состоянии батч оставил транзакцию, когда его пришлось откатить.
#[derive(Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TxLeftover {
    /// `BEGIN` без `COMMIT` — транзакция висела открытой.
    Open,
    /// Ошибка внутри явного `BEGIN … COMMIT`: `COMMIT` не выполнился, и
    /// транзакция осталась прерванной (любой запрос в ней — 25P02).
    Aborted,
}

impl TxLeftover {
    /// Предупреждение для человека (stderr CLI, текст ошибки MCP).
    pub fn warning(self, write: bool) -> String {
        let what = match self {
            TxLeftover::Open => "батч оставил транзакцию открытой (BEGIN без COMMIT)",
            TxLeftover::Aborted => {
                "батч упал внутри явной транзакции (BEGIN … COMMIT), COMMIT не выполнился"
            }
        };
        let effect = if write {
            " — выполнен ROLLBACK, изменения этой транзакции НЕ применены. \
             Батч sql-kai и так одна транзакция: BEGIN/COMMIT в нём не нужны"
        } else {
            " — выполнен ROLLBACK"
        };
        format!("{what}{effect}; сессия снова свободна")
    }
}

/// Строки для человека после батча: маркер выброшенных notices, сами notices,
/// предупреждение об откате. `error` — текст ошибки, которую покажут рядом:
/// если это и есть предупреждение (сервер отказал из-за отката), второй раз
/// его не печатаем.
pub fn server_message_lines(
    notices: &[Notice],
    dropped: u64,
    left: Option<TxLeftover>,
    write: bool,
    error: Option<&str>,
) -> Vec<String> {
    let mut out = Vec::new();
    if dropped > 0 {
        out.push(format!(
            "⚠ sql-kai: пропущено {dropped} сообщений сервера — буфер сессии переполнен, \
             выброшены самые ранние"
        ));
    }
    for n in notices {
        out.extend(n.lines());
    }
    if let Some(left) = left {
        let w = left.warning(write);
        if error != Some(w.as_str()) {
            out.push(format!("⚠ sql-kai: {w}"));
        }
    }
    out
}

/// Точное состояние транзакции на соединении — в отличие от эвристики
/// [`advance_tx`], которая не видит ни `COMMIT` внутри процедуры, ни ошибку,
/// пришедшую не от того стейтмента.
///
/// ReadyForQuery tokio-postgres наружу не отдаёт, поэтому спрашиваем сервер:
/// вне явного блока каждый запрос — своя неявная транзакция, и её начало
/// совпадает с началом стейтмента (StartTransaction берёт stmtStartTimestamp);
/// внутри открытого блока транзакция началась раньше. Прерванная отвечает
/// 25P02 на любой запрос, кроме ROLLBACK.
pub async fn probe_tx(client: &Client) -> Result<TxStatus, AppError> {
    match execute(
        client,
        "SELECT pg_catalog.statement_timestamp() = pg_catalog.transaction_timestamp()",
        1,
    )
    .await
    {
        Ok(exec) => {
            let idle = exec
                .results
                .first()
                .and_then(|r| r.rows.first())
                .is_some_and(|row| cell_bool(row, 0) || cell(row, 0) == "t");
            Ok(if idle {
                TxStatus::Idle
            } else {
                TxStatus::Active
            })
        }
        Err(e) if e.sqlstate() == Some(SqlState::IN_FAILED_SQL_TRANSACTION.code()) => {
            Ok(TxStatus::Failed)
        }
        Err(e) => Err(e),
    }
}

/// Возвращает соединение в idle: если транзакция открыта или прервана —
/// ROLLBACK. `Some` — что именно пришлось откатить; `Err` — соединение в
/// неизвестном состоянии, его надо выбросить.
pub async fn settle_tx(client: &Client) -> Result<Option<TxLeftover>, AppError> {
    let left = match probe_tx(client).await? {
        TxStatus::Idle => return Ok(None),
        TxStatus::Active => TxLeftover::Open,
        TxStatus::Failed => TxLeftover::Aborted,
    };
    execute(client, "ROLLBACK", 1).await?;
    Ok(Some(left))
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
        ..Default::default()
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

    /// [`settle_tx`] + Idle в трекере: после него соединение гарантированно
    /// вне транзакции (или `Err` — и тогда его надо выбросить).
    pub async fn settle(&self) -> Result<Option<TxLeftover>, AppError> {
        let left = settle_tx(self.client).await?;
        self.mark_idle();
        Ok(left)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(msg: &str) -> Notice {
        Notice {
            severity: "NOTICE".into(),
            message: msg.into(),
            detail: None,
            hint: None,
        }
    }

    #[test]
    fn notice_lines_follow_psql() {
        let n = Notice {
            detail: Some("d".into()),
            hint: Some("h".into()),
            ..notice("hello 42")
        };
        assert_eq!(n.lines(), ["NOTICE: hello 42", "DETAIL: d", "HINT: h"]);
    }

    #[test]
    fn notice_sink_drains_in_order_and_is_bounded() {
        let sink = NoticeSink::enabled();
        for i in 0..NOTICE_CAP + 5 {
            sink.push(notice(&i.to_string()));
        }
        let got = sink.take();
        assert_eq!(got.notices.len(), NOTICE_CAP);
        assert_eq!(got.notices[0].message, "5", "the oldest are dropped first");
        assert_eq!(got.dropped, 5);
        let again = sink.take();
        assert!(again.notices.is_empty() && again.dropped == 0);
    }

    /// Байтовые потолки: одно огромное сообщение режется, а поток больших
    /// не раздувает буфер дальше NOTICE_BUF_BYTES.
    #[test]
    fn notice_sink_is_bounded_by_bytes() {
        let sink = NoticeSink::enabled();
        for _ in 0..40 {
            sink.push(notice(&"y".repeat(NOTICE_FIELD_BYTES * 4)));
        }
        let got = sink.take();
        let bytes: usize = got.notices.iter().map(Notice::size).sum();
        assert!(bytes <= NOTICE_BUF_BYTES, "{bytes}");
        assert!(got.dropped > 0);
        assert!(got.notices[0].message.contains("more bytes cut"));
    }

    /// Выключенный буфер (вкладки GUI) не копит ничего.
    #[test]
    fn disabled_sink_keeps_nothing() {
        let sink = NoticeSink::default();
        sink.push(notice("x"));
        assert!(sink.take().notices.is_empty());
    }

    #[test]
    fn cap_notices_cuts_fields_and_drops_the_tail() {
        let mut list = vec![notice(&"a".repeat(100)), notice("b"), notice("c")];
        let (cut, dropped) = cap_notices(&mut list, 10, 30);
        assert!(cut);
        assert!(list[0]
            .message
            .starts_with("aaaaaaaaaa…[90 more bytes cut]"));
        assert_eq!(dropped, 2, "{list:?}");
        let mut small = vec![notice("ok")];
        assert_eq!(cap_notices(&mut small, 10, 30), (false, 0));
    }

    /// Отказ сервера из-за отката уже несёт текст предупреждения — второй
    /// раз его не печатаем; маркер пропущенных идёт первым.
    #[test]
    fn server_message_lines_dedupe_and_mark_drops() {
        let w = TxLeftover::Open.warning(true);
        let lines = server_message_lines(&[notice("n")], 3, Some(TxLeftover::Open), true, Some(&w));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("пропущено 3"));
        assert_eq!(lines[1], "NOTICE: n");
        let lines = server_message_lines(&[], 0, Some(TxLeftover::Open), true, Some("ERROR: x"));
        assert_eq!(lines, vec![format!("⚠ sql-kai: {w}")]);
    }

    /// Пустые notices и отсутствие отката не меняют форму ответа — старые
    /// потребители брокера и `--json` видят ровно прежние поля; ответ старого
    /// брокера без этих полей тоже читается.
    #[test]
    fn exec_result_new_fields_are_optional_on_the_wire() {
        let v = serde_json::to_value(ExecResult::default()).unwrap();
        assert_eq!(v, serde_json::json!({ "results": [], "durationMs": 0 }));
        let old: ExecResult =
            serde_json::from_value(serde_json::json!({ "results": [], "durationMs": 3 })).unwrap();
        assert!(old.notices.is_empty() && old.tx_rolled_back.is_none());

        let exec = ExecResult {
            notices: vec![notice("hi")],
            tx_rolled_back: Some(TxLeftover::Aborted),
            ..Default::default()
        };
        let v = serde_json::to_value(&exec).unwrap();
        assert_eq!(
            v["notices"],
            serde_json::json!([{ "severity": "NOTICE", "message": "hi" }])
        );
        assert_eq!(v["txRolledBack"], "aborted");
    }

    #[test]
    fn rollback_warning_says_writes_were_not_applied() {
        let w = TxLeftover::Aborted.warning(true);
        assert!(w.contains("ROLLBACK") && w.contains("НЕ применены"), "{w}");
        assert!(w.contains("BEGIN/COMMIT"), "{w}");
        let r = TxLeftover::Open.warning(false);
        assert!(r.contains("ROLLBACK") && !r.contains("НЕ применены"), "{r}");
    }
}
