use std::sync::atomic::AtomicU8;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

pub use tokio_postgres::types::Type;

use crate::error::AppError;
use crate::logging;
use crate::store::{self, Profile};
use crate::tunnel::{self, Tunnel};

pub struct Session {
    pub profile_id: String,
    /// For the diagnostics log — ids alone are unreadable there.
    pub profile_name: String,
    // Kept so a reloaded frontend can re-adopt live sessions (list_sessions).
    pub server_version: String,
    pub tunnel_port: Option<u16>,
    pub client: Arc<Client>,
    pub cancel: tokio_postgres::CancelToken,
    /// Heuristic transaction state (a [`TxStatus`] as u8), advanced after every
    /// `execute` on this connection. Arc so `execute_sql` can update it after
    /// the await without re-locking the session map.
    pub tx: Arc<AtomicU8>,
    /// A per-tab secondary connection (own backend pid / transaction) that
    /// reuses the profile's tunnel — not the profile's primary session.
    pub isolated: bool,
    // Held so the ssh child stays alive; killed on Drop.
    pub _tunnel: Option<Tunnel>,
    conn_task: tokio::task::JoinHandle<()>,
}

/// Connection-level transaction state. Tracked heuristically from the SQL we
/// run: tokio-postgres discards the protocol's ReadyForQuery status byte, so we
/// can't read it authoritatively. Advisory — drives the status-bar badge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TxStatus {
    /// Not in a transaction block (autocommit).
    Idle = 0,
    /// Inside an open transaction — BEGIN with no COMMIT/ROLLBACK yet.
    Active = 1,
    /// Transaction aborted by an error — every statement errors until ROLLBACK.
    Failed = 2,
}

impl TxStatus {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => TxStatus::Active,
            2 => TxStatus::Failed,
            _ => TxStatus::Idle,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            TxStatus::Idle => "idle",
            TxStatus::Active => "active",
            TxStatus::Failed => "failed",
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.conn_task.abort();
    }
}

pub struct Connected {
    pub session: Session,
    pub server_version: String,
    pub tunnel_port: Option<u16>,
}

/// Knobs for [`connect`]. Defaults (all None) = pull secrets from the vault,
/// no ssh multiplexing — the GUI's behavior.
#[derive(Default)]
pub struct ConnectOptions {
    /// Use this DB password instead of the vault's (`--password-env`, tests).
    pub password_override: Option<String>,
    /// Use this SSH key passphrase instead of the vault's.
    pub ssh_passphrase_override: Option<String>,
    /// Some(ttl) → reuse ssh auth via a persistent ControlMaster that lingers
    /// `ttl` seconds idle (CLI). None → standalone tunnel (GUI).
    pub ssh_mux_ttl: Option<u32>,
    /// Some((host, port)) → connect straight to this endpoint and open NO tunnel
    /// of our own (a secondary/isolated connection reusing the primary session's
    /// tunnel). None → normal behavior.
    pub endpoint_override: Option<(String, u16)>,
}

pub async fn connect(profile: &Profile, opts: ConnectOptions) -> Result<Connected, AppError> {
    // A secondary (isolated) connection reuses an existing tunnel's local
    // endpoint instead of opening its own ssh child.
    let isolated = opts.endpoint_override.is_some();
    let (host, port, tunnel) = if let Some((host, port)) = opts.endpoint_override {
        (host, port, None)
    } else {
        let tunnel = match &profile.ssh {
            Some(ssh) if !ssh.host.trim().is_empty() => {
                let passphrase = opts
                    .ssh_passphrase_override
                    .or_else(|| store::get_ssh_passphrase(profile));
                let tunnel = tunnel::open_tunnel(
                    ssh,
                    &profile.host,
                    profile.port,
                    passphrase.as_deref(),
                    opts.ssh_mux_ttl,
                )
                .await
                .inspect_err(|e| {
                    logging::log(
                        "connect",
                        &format!("\"{}\": ssh tunnel failed: {e}", profile.name),
                    );
                })?;
                Some(tunnel)
            }
            _ => None,
        };
        let (host, port) = match &tunnel {
            Some(t) => ("127.0.0.1".to_string(), t.local_port),
            None => (profile.host.clone(), profile.port),
        };
        (host, port, tunnel)
    };

    let mut cfg = tokio_postgres::Config::new();
    cfg.host(&host)
        .port(port)
        .user(&profile.user)
        .dbname(&profile.database)
        .application_name("sql-kai")
        .connect_timeout(Duration::from_secs(10));
    let password = opts.password_override.or_else(|| store::get_password(profile));
    if let Some(pw) = password.filter(|p| !p.is_empty()) {
        cfg.password(&pw);
    }

    let (client, connection) = cfg.connect(NoTls).await.inspect_err(|e| {
        logging::log(
            "connect",
            &format!("\"{}\": pg connect to {host}:{port} failed: {e}", profile.name),
        );
    })?;
    // The connection future resolves when the wire dies — its resolution is
    // the ground truth for "why did this session drop".
    let log_name = profile.name.clone();
    let conn_task = tokio::spawn(async move {
        match connection.await {
            Ok(()) => logging::log(
                "session",
                &format!("\"{log_name}\": connection closed by server or tunnel"),
            ),
            Err(e) => logging::log(
                "session",
                &format!("\"{log_name}\": connection terminated: {e}"),
            ),
        }
    });
    let client = Arc::new(client);
    let cancel = client.cancel_token();

    let server_version = match client.simple_query("SHOW server_version").await {
        Ok(msgs) => msgs
            .iter()
            .find_map(|m| match m {
                SimpleQueryMessage::Row(r) => r.get(0).map(|s| s.to_string()),
                _ => None,
            })
            .unwrap_or_default(),
        Err(_) => String::new(),
    };

    let tunnel_port = tunnel.as_ref().map(|t| t.local_port);
    logging::log(
        "connect",
        &format!(
            "\"{}\": connected to {}:{}{} (PostgreSQL {server_version})",
            profile.name,
            profile.host,
            profile.port,
            tunnel_port
                .map(|p| format!(" via ssh tunnel 127.0.0.1:{p}"))
                .unwrap_or_default(),
        ),
    );
    Ok(Connected {
        session: Session {
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            server_version: server_version.clone(),
            tunnel_port,
            client,
            cancel,
            tx: Arc::new(AtomicU8::new(TxStatus::Idle as u8)),
            isolated,
            _tunnel: tunnel,
            conn_task,
        },
        server_version,
        tunnel_port,
    })
}

#[derive(Serialize, Clone, Debug, Default)]
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

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub results: Vec<StatementResult>,
    pub duration_ms: u64,
}

/// Runs SQL through the simple-query protocol: multiple `;`-separated statements
/// are supported and every value arrives already text-formatted by the server.
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

/// Cell text at `i`; "" for NULL or a missing column.
pub fn cell(row: &[Option<String>], i: usize) -> String {
    row.get(i).cloned().flatten().unwrap_or_default()
}

/// Boolean cell rendered by Postgres as "true"/"false".
pub fn cell_bool(row: &[Option<String>], i: usize) -> bool {
    row.get(i).and_then(|v| v.as_deref()) == Some("true")
}

/// All rows of every result — for catalog queries.
pub async fn query_rows(
    client: &Client,
    sql: &str,
) -> Result<Vec<Vec<Option<String>>>, AppError> {
    let exec = execute(client, sql, 10_000).await?;
    Ok(exec.results.into_iter().flat_map(|r| r.rows).collect())
}

/// First column of the first row — for single-value catalog lookups.
pub async fn query_scalar(client: &Client, sql: &str) -> Result<Option<String>, AppError> {
    Ok(query_rows(client, sql).await?.first().and_then(|r| r[0].clone()))
}

/// Splits multi-statement SQL on `;`, honoring '…'/E'…' strings, "…"
/// identifiers, $tag$…$tag$ dollar quoting and --/nested /* */ comments.
/// Statements that are only whitespace/comments are dropped — the simple-query
/// protocol yields no result for them, keeping this 1:1 with [`execute`].
pub fn split_statements(sql: &str) -> Vec<String> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut start = 0; // byte offset where the current statement begins
    let mut has_token = false; // current statement has non-comment content
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 1;
                i += 2;
                while i < b.len() && depth > 0 {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'\'' => {
                // E'…' also understands backslash escapes
                let estring = i > 0
                    && (b[i - 1] == b'e' || b[i - 1] == b'E')
                    && (i < 2 || !(b[i - 2].is_ascii_alphanumeric() || b[i - 2] == b'_'));
                has_token = true;
                i += 1;
                while i < b.len() {
                    if estring && b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == b'\'' {
                        if b.get(i + 1) == Some(&b'\'') {
                            i += 2; // '' — escaped quote
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'"' => {
                has_token = true;
                i += 1;
                while i < b.len() {
                    if b[i] == b'"' {
                        if b.get(i + 1) == Some(&b'"') {
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'$' => {
                // $tag$ … $tag$; $1 (a digit after $) is a parameter, not a tag
                let mut j = i + 1;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                let is_tag = j < b.len()
                    && b[j] == b'$'
                    && (j == i + 1 || !b[i + 1].is_ascii_digit());
                has_token = true;
                if is_tag {
                    let tag = &b[i..=j];
                    i = j + 1;
                    while i + tag.len() <= b.len() && &b[i..i + tag.len()] != tag {
                        i += 1;
                    }
                    i = (i + tag.len()).min(b.len());
                } else {
                    i += 1;
                }
            }
            b';' => {
                if has_token {
                    out.push(sql[start..i].trim().to_string());
                }
                start = i + 1;
                has_token = false;
                i += 1;
            }
            c => {
                if !c.is_ascii_whitespace() {
                    has_token = true;
                }
                i += 1;
            }
        }
    }
    if has_token {
        out.push(sql[start..].trim().to_string());
    }
    out
}

/// Skips leading whitespace and SQL comments so keyword sniffing sees the
/// actual command (`-- note\nBEGIN` -> `BEGIN`).
fn strip_leading_noise(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix("--") {
            s = rest.split_once('\n').map_or("", |(_, r)| r);
        } else if s.starts_with("/*") {
            match s.find("*/") {
                Some(i) => s = &s[i + 2..],
                None => return "",
            }
        } else {
            return s;
        }
    }
}

/// The first two SQL keywords of a statement, uppercased (command + qualifier).
fn head_keywords(stmt: &str) -> (String, String) {
    let mut it = strip_leading_noise(stmt)
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_uppercase());
    (it.next().unwrap_or_default(), it.next().unwrap_or_default())
}

/// Advances the tracked transaction status after running `sql` (which produced
/// `ok`). On success the outcome is exact — the whole batch ran (simple_query
/// returns Ok only then), so we fold the transaction verbs over its statements.
/// On failure we can't tell which statement broke, so we err toward "aborted"
/// (the safe nudge to ROLLBACK). Heuristic: exotic cases (COMMIT inside a
/// procedure, a partial multi-transaction batch) may mis-track.
pub fn advance_tx(cur: TxStatus, sql: &str, ok: bool) -> TxStatus {
    if !ok {
        let opens_tx = || {
            split_statements(sql)
                .iter()
                .any(|s| matches!(head_keywords(s).0.as_str(), "BEGIN" | "START"))
        };
        return match cur {
            // an implicit tx rolls back to idle; an explicit BEGIN that failed
            // partway leaves the connection in an aborted, still-open tx
            TxStatus::Idle if opens_tx() => TxStatus::Failed,
            TxStatus::Idle => TxStatus::Idle,
            _ => TxStatus::Failed,
        };
    }
    let mut s = cur;
    for stmt in split_statements(sql) {
        let (w0, w1) = head_keywords(&stmt);
        s = match w0.as_str() {
            "BEGIN" | "START" => TxStatus::Active,
            "COMMIT" | "END" | "ABORT" => TxStatus::Idle,
            // ROLLBACK TO [SAVEPOINT] keeps the tx open (and un-aborts it);
            // plain ROLLBACK ends it
            "ROLLBACK" if w1 == "TO" => {
                if s == TxStatus::Failed {
                    TxStatus::Active
                } else {
                    s
                }
            }
            "ROLLBACK" => TxStatus::Idle,
            _ => s,
        };
    }
    s
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

pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// quote_literal()'d qualified name, ready for a `::regclass` cast.
pub fn regclass_literal(schema: &str, table: &str) -> String {
    quote_literal(&format!("{}.{}", quote_ident(schema), quote_ident(table)))
}

pub const TABLES_SQL: &str = "\
SELECT n.nspname, c.relname, c.relkind::text
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r','p','v','m','f')
  AND n.nspname NOT IN ('pg_catalog','information_schema')
  AND n.nspname NOT LIKE 'pg_toast%'
  AND n.nspname NOT LIKE 'pg_temp%'
ORDER BY n.nspname, c.relname";

/// TABLES_SQL + a 4th column: approximate row count from pg_class.reltuples.
/// '?' = never analyzed (reltuples = -1); partitioned parents sum their
/// direct partitions; views get NULL.
pub const TABLES_COUNTS_SQL: &str = "\
SELECT n.nspname, c.relname, c.relkind::text,
       CASE
         WHEN c.relkind = 'p' THEN COALESCE((
             SELECT sum(GREATEST(ch.reltuples, 0))::bigint
             FROM pg_inherits i JOIN pg_class ch ON ch.oid = i.inhrelid
             WHERE i.inhparent = c.oid), 0)::text
         WHEN c.relkind IN ('r','m','f') THEN
           CASE WHEN c.reltuples < 0 THEN '?' ELSE c.reltuples::bigint::text END
       END
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r','p','v','m','f')
  AND n.nspname NOT IN ('pg_catalog','information_schema')
  AND n.nspname NOT LIKE 'pg_toast%'
  AND n.nspname NOT LIKE 'pg_temp%'
ORDER BY n.nspname, c.relname";

/// `regclass` — a quote_literal()'d qualified table name, e.g. `'"public"."t"'`.
pub fn columns_sql(regclass: &str) -> String {
    format!(
        "SELECT a.attname,
                format_type(a.atttypid, a.atttypmod),
                (NOT a.attnotnull)::text,
                COALESCE((SELECT true FROM pg_index i
                          WHERE i.indrelid = a.attrelid
                            AND a.attnum = ANY(i.indkey)
                            AND i.indisprimary), false)::text,
                pg_get_expr(d.adbin, d.adrelid),
                col_description(a.attrelid, a.attnum)
         FROM pg_attribute a
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
         WHERE a.attrelid = {regclass}::regclass AND a.attnum > 0 AND NOT a.attisdropped
         ORDER BY a.attnum"
    )
}

pub fn indexes_sql(regclass: &str) -> String {
    format!(
        "SELECT c.relname,
                i.indisunique::text,
                i.indisprimary::text,
                (SELECT string_agg(a.attname, ', ' ORDER BY k.ord)
                   FROM unnest(i.indkey) WITH ORDINALITY AS k(attnum, ord)
                   JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum
                  WHERE k.attnum > 0),
                pg_get_indexdef(i.indexrelid)
         FROM pg_index i
         JOIN pg_class c ON c.oid = i.indexrelid
         WHERE i.indrelid = {regclass}::regclass
         ORDER BY i.indisprimary DESC, c.relname"
    )
}

pub fn relations_sql(regclass: &str) -> String {
    format!(
        "SELECT con.conname,
                (SELECT string_agg(a.attname, ', ' ORDER BY k.ord)
                   FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord)
                   JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum),
                con.confrelid::regclass::text,
                (SELECT string_agg(a.attname, ', ' ORDER BY k.ord)
                   FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord)
                   JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum),
                CASE con.confupdtype WHEN 'a' THEN 'NO ACTION' WHEN 'r' THEN 'RESTRICT'
                     WHEN 'c' THEN 'CASCADE' WHEN 'n' THEN 'SET NULL' WHEN 'd' THEN 'SET DEFAULT'
                     ELSE con.confupdtype::text END,
                CASE con.confdeltype WHEN 'a' THEN 'NO ACTION' WHEN 'r' THEN 'RESTRICT'
                     WHEN 'c' THEN 'CASCADE' WHEN 'n' THEN 'SET NULL' WHEN 'd' THEN 'SET DEFAULT'
                     ELSE con.confdeltype::text END
         FROM pg_constraint con
         WHERE con.conrelid = {regclass}::regclass AND con.contype = 'f'
         ORDER BY con.conname"
    )
}

pub fn triggers_sql(regclass: &str) -> String {
    format!(
        "SELECT t.tgname,
                CASE WHEN (t.tgtype & 2) > 0 THEN 'BEFORE'
                     WHEN (t.tgtype & 64) > 0 THEN 'INSTEAD OF'
                     ELSE 'AFTER' END,
                concat_ws(' OR ',
                  CASE WHEN (t.tgtype & 4) > 0 THEN 'INSERT' END,
                  CASE WHEN (t.tgtype & 8) > 0 THEN 'DELETE' END,
                  CASE WHEN (t.tgtype & 16) > 0 THEN 'UPDATE' END,
                  CASE WHEN (t.tgtype & 32) > 0 THEN 'TRUNCATE' END),
                pg_get_triggerdef(t.oid)
         FROM pg_trigger t
         WHERE t.tgrelid = {regclass}::regclass AND NOT t.tgisinternal
         ORDER BY t.tgname"
    )
}

/** Postgres has no SHOW CREATE TABLE — assemble the DDL from the catalogs:
 *  columns (types/defaults/identity), constraints, secondary indexes,
 *  partition key and comments. Views return their stored definition. */
pub async fn table_ddl(client: &Client, schema: &str, table: &str) -> Result<String, AppError> {
    let rel = format!("{}::regclass", regclass_literal(schema, table));
    let qualified = format!("{}.{}", quote_ident(schema), quote_ident(table));

    let kind = query_scalar(
        client,
        &format!("SELECT relkind::text FROM pg_class WHERE oid = {rel}"),
    )
    .await?
    .unwrap_or_default();

    if kind == "v" || kind == "m" {
        let body = query_scalar(client, &format!("SELECT pg_get_viewdef({rel}, true)"))
            .await?
            .unwrap_or_default();
        let head = if kind == "m" {
            "CREATE MATERIALIZED VIEW"
        } else {
            "CREATE OR REPLACE VIEW"
        };
        return Ok(format!("{head} {qualified} AS\n{body}"));
    }

    let cols = query_rows(
        client,
        &format!(
            "SELECT a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull::text, \
             pg_get_expr(d.adbin, d.adrelid), a.attidentity::text, a.attgenerated::text \
             FROM pg_attribute a \
             LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attrelid = {rel} AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum"
        ),
    )
    .await?;
    let mut lines: Vec<String> = Vec::new();
    for row in &cols {
        let mut line = format!("  {} {}", quote_ident(&cell(row, 0)), cell(row, 1));
        let default = row[3].as_deref();
        match (row[5].as_deref(), row[4].as_deref()) {
            (Some("s"), _) => line.push_str(&format!(
                " GENERATED ALWAYS AS ({}) STORED",
                default.unwrap_or_default()
            )),
            (_, Some("a")) => line.push_str(" GENERATED ALWAYS AS IDENTITY"),
            (_, Some("d")) => line.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
            _ => {
                if let Some(d) = default {
                    line.push_str(&format!(" DEFAULT {d}"));
                }
            }
        }
        if cell_bool(row, 2) {
            line.push_str(" NOT NULL");
        }
        lines.push(line);
    }

    // NOT NULL lives inline above — 'n' rows (PG18+) would duplicate it.
    let cons = query_rows(
        client,
        &format!(
            "SELECT conname, pg_get_constraintdef(oid, true) FROM pg_constraint \
             WHERE conrelid = {rel} AND contype IN ('p','u','f','c','x') \
             ORDER BY CASE contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'f' THEN 2 ELSE 3 END, conname"
        ),
    )
    .await?;
    for row in &cons {
        lines.push(format!(
            "  CONSTRAINT {} {}",
            quote_ident(&cell(row, 0)),
            cell(row, 1)
        ));
    }

    let mut ddl = format!("CREATE TABLE {qualified} (\n{}\n)", lines.join(",\n"));
    if kind == "p" {
        if let Some(part) =
            query_scalar(client, &format!("SELECT pg_get_partkeydef({rel})")).await?
        {
            ddl.push_str(&format!(" PARTITION BY {part}"));
        }
    }
    ddl.push(';');

    let idx = query_rows(
        client,
        &format!(
            "SELECT pg_get_indexdef(i.indexrelid, 0, true) FROM pg_index i \
             WHERE i.indrelid = {rel} \
             AND NOT EXISTS (SELECT 1 FROM pg_constraint co WHERE co.conindid = i.indexrelid) \
             ORDER BY 1"
        ),
    )
    .await?;
    for row in &idx {
        if let Some(def) = row[0].as_deref() {
            ddl.push_str(&format!("\n{def};"));
        }
    }

    if let Some(c) =
        query_scalar(client, &format!("SELECT obj_description({rel}, 'pg_class')")).await?
    {
        ddl.push_str(&format!(
            "\nCOMMENT ON TABLE {qualified} IS {};",
            quote_literal(&c)
        ));
    }
    let comments = query_rows(
        client,
        &format!(
            "SELECT a.attname, col_description(a.attrelid, a.attnum) \
             FROM pg_attribute a \
             WHERE a.attrelid = {rel} AND a.attnum > 0 AND NOT a.attisdropped \
               AND col_description(a.attrelid, a.attnum) IS NOT NULL \
             ORDER BY a.attnum"
        ),
    )
    .await?;
    for row in &comments {
        if let Some(c) = row[1].as_deref() {
            ddl.push_str(&format!(
                "\nCOMMENT ON COLUMN {qualified}.{} IS {};",
                quote_ident(&cell(row, 0)),
                quote_literal(c)
            ));
        }
    }

    Ok(ddl)
}

#[cfg(test)]
mod tests {
    use super::split_statements;

    #[test]
    fn splits_plain_statements() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2;"),
            vec!["SELECT 1", "SELECT 2"]
        );
        // trailing statement without ';'
        assert_eq!(split_statements("SELECT 1"), vec!["SELECT 1"]);
    }

    #[test]
    fn semicolons_inside_quoting_do_not_split() {
        assert_eq!(
            split_statements("SELECT 'a;b'; SELECT \";\""),
            vec!["SELECT 'a;b'", "SELECT \";\""]
        );
        // '' escape inside a string
        assert_eq!(
            split_statements("SELECT 'it''s; fine'"),
            vec!["SELECT 'it''s; fine'"]
        );
        // E'' with a backslash-escaped quote
        assert_eq!(
            split_statements(r"SELECT E'a\';b'; SELECT 2"),
            vec![r"SELECT E'a\';b'", "SELECT 2"]
        );
        // dollar quoting, tagged and bare
        assert_eq!(
            split_statements("SELECT $$x;y$$; SELECT $fn$a;b$fn$"),
            vec!["SELECT $$x;y$$", "SELECT $fn$a;b$fn$"]
        );
    }

    #[test]
    fn comments_are_opaque_and_empty_statements_dropped() {
        assert_eq!(
            split_statements("SELECT 1 -- ; not a split\n; SELECT 2"),
            vec!["SELECT 1 -- ; not a split", "SELECT 2"]
        );
        assert_eq!(
            split_statements("/* a; /* nested; */ b; */ SELECT 1"),
            vec!["/* a; /* nested; */ b; */ SELECT 1"]
        );
        // comment-only / whitespace-only chunks yield no statement
        assert_eq!(split_statements("-- only a comment\n; ;"), Vec::<String>::new());
        assert_eq!(split_statements(";;  ;"), Vec::<String>::new());
    }

    #[test]
    fn dollar_parameter_is_not_a_tag() {
        assert_eq!(
            split_statements("SELECT $1; SELECT 2"),
            vec!["SELECT $1", "SELECT 2"]
        );
    }

    #[test]
    fn tracks_transaction_status() {
        use super::advance_tx;
        use super::TxStatus::{Active, Failed, Idle};
        // open, then close
        assert_eq!(advance_tx(Idle, "BEGIN", true), Active);
        assert_eq!(advance_tx(Active, "COMMIT", true), Idle);
        assert_eq!(advance_tx(Active, "ROLLBACK", true), Idle);
        // a whole cycle in one batch nets out to idle
        assert_eq!(advance_tx(Idle, "BEGIN; UPDATE t SET x=1; COMMIT", true), Idle);
        // BEGIN left open across runs stays active
        assert_eq!(advance_tx(Idle, "BEGIN; SELECT 1", true), Active);
        // an error inside an open tx aborts it; in autocommit it stays idle
        assert_eq!(advance_tx(Active, "SELECT bad", false), Failed);
        assert_eq!(advance_tx(Idle, "SELECT bad", false), Idle);
        // BEGIN then an error -> aborted, tx still open
        assert_eq!(advance_tx(Idle, "BEGIN; SELECT bad", false), Failed);
        // recovery: ROLLBACK clears the aborted state; other stmts keep failing
        assert_eq!(advance_tx(Failed, "ROLLBACK", true), Idle);
        assert_eq!(advance_tx(Failed, "SELECT 1", false), Failed);
        // ROLLBACK TO SAVEPOINT un-aborts but keeps the tx open
        assert_eq!(advance_tx(Failed, "ROLLBACK TO SAVEPOINT sp", true), Active);
        // synonyms and a leading comment
        assert_eq!(advance_tx(Active, "END", true), Idle);
        assert_eq!(advance_tx(Idle, "START TRANSACTION", true), Active);
        assert_eq!(advance_tx(Active, "ABORT", true), Idle);
        assert_eq!(advance_tx(Idle, "-- go\nBEGIN", true), Active);
        // a DO block's inner BEGIN/END is dollar-quoted, not a transaction verb
        assert_eq!(advance_tx(Idle, "DO $$ BEGIN PERFORM 1; END $$", true), Idle);
    }
}
