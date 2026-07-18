//! `sql-kai mcp <alias>` — MCP-сервер (stdio) для AI-агентов: структурные
//! tools вместо разбора текстового вывода CLI. Панель агента в GUI передаёт
//! эту команду в ACP `session/new` (mcpServers), но сервер самодостаточен —
//! его можно подключить к любому MCP-клиенту.
//!
//! Все запросы идут через сервер сессий (GUI-брокер или holder): vault уже
//! разблокирован, туннели подняты, сессия read-only по умолчанию. stdout —
//! канал протокола (newline-delimited JSON-RPC), диагностика — в stderr.

use std::process::ExitCode;

use clap::Args;
use serde_json::{json, Value};
use sql_kai_lib::db;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, HistoryEntry, Profile};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::cmd::introspect::split_table;
use crate::{broker_client, redact, session};

/// Версия MCP, которую сервер заявляет, если клиентскую не знает.
const PROTOCOL_FALLBACK: &str = "2025-06-18";
/// Строк на запрос по умолчанию — меньше, чем в CLI: ответ читает модель.
const DEFAULT_MAX_ROWS: usize = 200;

#[derive(Args)]
pub struct McpArgs {
    /// Профиль: имя, id или группа
    alias: String,
}

pub async fn run(a: McpArgs) -> Result<ExitCode, AppError> {
    let profile = session::resolve_profile(&a.alias)?;
    eprintln!(
        "sql-kai mcp: профиль {} ({} @ {}:{})",
        profile.name, profile.database, profile.host, profile.port
    );

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Some(reply) = handle_line(&profile, &line).await else {
            continue; // notification — ответа не положено
        };
        let mut buf = serde_json::to_vec(&reply).unwrap_or_else(|_| b"{}".to_vec());
        buf.push(b'\n');
        stdout.write_all(&buf).await?;
        stdout.flush().await?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Одно сообщение протокола → ответ (None для notification'ов).
async fn handle_line(profile: &Profile, line: &str) -> Option<Value> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(json!({
                "jsonrpc": "2.0", "id": Value::Null,
                "error": { "code": -32700, "message": format!("parse error: {e}") },
            }));
        }
    };
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    // notification (без id) — обрабатывать нечего: сервер не ждёт
    // notifications/initialized и не имеет подписок
    let id = id?;

    let result = match method {
        "initialize" => Ok(initialize_result(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => return Some(tools_call(profile, &id, &params).await),
        _ => Err(format!("method not supported: {method}")),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(message) => json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": -32601, "message": message },
        }),
    })
}

fn initialize_result(params: &Value) -> Value {
    // отвечаем версией клиента: серверу с одними tools совместимы все
    // известные ревизии, а несуществующую строку клиент отвергнет сам
    let version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_FALLBACK);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "sql-kai", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tool_definitions() -> Value {
    let table_schema = json!({
        "type": "object",
        "properties": {
            "table": { "type": "string", "description": "Table as [schema.]name; schema defaults to public" }
        },
        "required": ["table"],
    });
    json!([
        {
            "name": "query",
            "description": "Run SQL in the connected PostgreSQL database. The session is READ-ONLY unless `write` is set — set it only when the user explicitly asked to modify data. Sensitive-looking columns (password/secret/*_token/*_key) are masked in the output.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "SQL to execute; multiple ;-separated statements allowed" },
                    "maxRows": { "type": "integer", "description": "Row cap per statement (default 200)" },
                    "write": { "type": "boolean", "description": "Allow writes for this call (default false)" }
                },
                "required": ["sql"],
            },
        },
        {
            "name": "tables",
            "description": "List tables, views and materialized views of the database.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "counts": { "type": "boolean", "description": "Include approximate row counts (default false)" }
                },
            },
        },
        { "name": "columns", "description": "Columns of a table: name, type, nullability, PK, default, comment.", "inputSchema": table_schema },
        { "name": "ddl", "description": "CREATE TABLE / CREATE VIEW statement of a table or view.", "inputSchema": table_schema },
        { "name": "indexes", "description": "Indexes of a table: name, uniqueness, columns, definition.", "inputSchema": table_schema },
        {
            "name": "open_table",
            "description": "Show a table to the user: opens the table as a tab in the sql-kai GUI. Use when the user asks to 'show'/'open' a table.",
            "inputSchema": table_schema,
        },
        {
            "name": "open_query",
            "description": "Open a query tab in the sql-kai GUI pre-filled with the given SQL, WITHOUT executing it. Use to hand a prepared query over to the user.",
            "inputSchema": {
                "type": "object",
                "properties": { "sql": { "type": "string" } },
                "required": ["sql"],
            },
        },
        {
            "name": "selection",
            "description": "What the user currently sees in the sql-kai GUI: the active tab (table with its filter/sort/page, or query with its SQL) and the rows, columns or cells they have selected — with the selected data. Use when the user refers to what's on their screen: 'this row', 'the selected rows', 'this column', 'что я выделил' etc.",
            "inputSchema": { "type": "object", "properties": {} },
        },
    ])
}

/// tools/call: любые ошибки — внутрь result (isError), а не в error протокола,
/// чтобы модель видела текст и могла отреагировать.
async fn tools_call(profile: &Profile, id: &Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let outcome = dispatch_tool(profile, name, &args).await;
    let (text, is_error) = match outcome {
        Ok(text) => (text, false),
        Err(message) => (message, true),
    };
    json!({
        "jsonrpc": "2.0", "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "isError": is_error,
        },
    })
}

async fn dispatch_tool(profile: &Profile, name: &str, args: &Value) -> Result<String, String> {
    let table_arg = || -> Result<(String, String), String> {
        let spec = args
            .get("table")
            .and_then(Value::as_str)
            .ok_or("missing required argument: table")?;
        Ok(split_table(spec))
    };
    match name {
        "query" => {
            let sql = args
                .get("sql")
                .and_then(Value::as_str)
                .ok_or("missing required argument: sql")?;
            let max_rows = args
                .get("maxRows")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(DEFAULT_MAX_ROWS);
            let write = args.get("write").and_then(Value::as_bool).unwrap_or(false);
            run_query(profile, sql, max_rows, write, true).await
        }
        "tables" => {
            let counts = args.get("counts").and_then(Value::as_bool).unwrap_or(false);
            let sql = if counts { db::TABLES_COUNTS_SQL } else { db::TABLES_SQL };
            run_query(profile, sql, 10_000, false, false).await
        }
        "columns" => {
            let (schema, table) = table_arg()?;
            let sql = db::columns_sql(&db::regclass_literal(&schema, &table));
            run_query(profile, &sql, 10_000, false, false).await
        }
        "indexes" => {
            let (schema, table) = table_arg()?;
            let sql = db::indexes_sql(&db::regclass_literal(&schema, &table));
            run_query(profile, &sql, 10_000, false, false).await
        }
        "ddl" => {
            let (schema, table) = table_arg()?;
            let mut b = connect_broker().await?;
            b.ddl(&profile.id, &schema, &table)
                .await
                .map_err(|e| e.to_string())
        }
        "open_table" => {
            let (schema, table) = table_arg()?;
            let mut b = connect_broker().await?;
            b.open_table(&profile.id, &schema, &table)
                .await
                .map(|_| format!("opened {schema}.{table} in a sql-kai tab"))
                .map_err(|e| e.to_string())
        }
        "open_query" => {
            let sql = args
                .get("sql")
                .and_then(Value::as_str)
                .ok_or("missing required argument: sql")?;
            let mut b = connect_broker().await?;
            b.open_query(&profile.id, sql)
                .await
                .map(|_| "opened a query tab in sql-kai (not executed)".to_string())
                .map_err(|e| e.to_string())
        }
        "selection" => {
            let mut b = connect_broker().await?;
            b.gui_selection(&profile.id).await.map_err(|e| e.to_string())
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

async fn connect_broker() -> Result<broker_client::BrokerClient, String> {
    broker_client::connect_any().await.ok_or_else(|| {
        "нет сервера сессий: запусти приложение sql-kai (или настрой тихую \
         разблокировку vault: sql-kai vault trust)"
            .to_string()
    })
}

/// SQL через сервер сессий; результат — компактный JSON (все значения
/// строками, как отдаёт simple-query протокол). `record` — писать в историю
/// (пользовательские запросы; интроспекция историю не засоряет).
async fn run_query(
    profile: &Profile,
    sql: &str,
    max_rows: usize,
    write: bool,
    record: bool,
) -> Result<String, String> {
    let mut b = connect_broker().await?;
    let outcome = b.query(&profile.id, sql, max_rows.max(1), write, false).await;
    if record {
        let _ = store::record_history(HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            sql: sql.to_string(),
            at: store::now_ms(),
            ok: outcome.is_ok(),
        });
    }
    let mut exec = outcome.map_err(|e| e.to_string())?.exec;
    let masked = redact::redact_exec(&mut exec);
    let mut payload = serde_json::to_value(&exec).map_err(|e| e.to_string())?;
    if !masked.is_empty() {
        payload["maskedColumns"] = json!(masked);
    }
    serde_json::to_string(&payload).map_err(|e| e.to_string())
}
