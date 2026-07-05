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

pub async fn connect(
    profile: &Profile,
    password_override: Option<String>,
    ssh_passphrase_override: Option<String>,
) -> Result<Connected, AppError> {
    let tunnel = match &profile.ssh {
        Some(ssh) if !ssh.host.trim().is_empty() => {
            let passphrase =
                ssh_passphrase_override.or_else(|| store::get_ssh_passphrase(profile));
            Some(
                tunnel::open_tunnel(ssh, &profile.host, profile.port, passphrase.as_deref())
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
        .application_name("sql-tauri")
        .connect_timeout(Duration::from_secs(10));
    let password = password_override.or_else(|| store::get_password(profile));
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

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StatementResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub rows_affected: Option<u64>,
    pub truncated: bool,
}

impl StatementResult {
    fn new() -> Self {
        StatementResult {
            columns: vec![],
            rows: vec![],
            rows_affected: None,
            truncated: false,
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
                let mut res = StatementResult::new();
                res.columns = cols.iter().map(|c| c.name().to_string()).collect();
                current = Some(res);
            }
            SimpleQueryMessage::Row(row) => {
                let cur = current.get_or_insert_with(|| {
                    let mut res = StatementResult::new();
                    res.columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                    res
                });
                if cur.rows.len() < max_rows {
                    cur.rows
                        .push((0..row.len()).map(|i| row.get(i).map(str::to_string)).collect());
                } else {
                    cur.truncated = true;
                }
            }
            SimpleQueryMessage::CommandComplete(n) => {
                let mut res = current.take().unwrap_or_else(StatementResult::new);
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

pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

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
