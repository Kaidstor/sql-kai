use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

use crate::error::AppError;
use crate::store::{self, Profile};
use crate::tunnel::{self, Tunnel};

pub struct Session {
    pub profile_id: String,
    // Kept so a reloaded frontend can re-adopt live sessions (list_sessions).
    pub server_version: String,
    pub tunnel_port: Option<u16>,
    pub client: Arc<Client>,
    pub cancel: tokio_postgres::CancelToken,
    // Held so the ssh child stays alive; killed on Drop.
    pub _tunnel: Option<Tunnel>,
    conn_task: tokio::task::JoinHandle<()>,
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
}

pub async fn connect(profile: &Profile, opts: ConnectOptions) -> Result<Connected, AppError> {
    let tunnel = match &profile.ssh {
        Some(ssh) if !ssh.host.trim().is_empty() => {
            let passphrase = opts
                .ssh_passphrase_override
                .or_else(|| store::get_ssh_passphrase(profile));
            Some(
                tunnel::open_tunnel(
                    ssh,
                    &profile.host,
                    profile.port,
                    passphrase.as_deref(),
                    opts.ssh_mux_ttl,
                )
                .await?,
            )
        }
        _ => None,
    };
    let (host, port) = match &tunnel {
        Some(t) => ("127.0.0.1".to_string(), t.local_port),
        None => (profile.host.clone(), profile.port),
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

    let (client, connection) = cfg.connect(NoTls).await?;
    let conn_task = tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("pg connection error: {e}");
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
    Ok(Connected {
        session: Session {
            profile_id: profile.id.clone(),
            server_version: server_version.clone(),
            tunnel_port,
            client,
            cancel,
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
