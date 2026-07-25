//! `sql-kai mcp [alias]` — MCP-сервер (stdio) для AI-агентов: структурные
//! tools вместо разбора текстового вывода CLI. Панель агента в GUI передаёт
//! `mcp <profileId>` в ACP `session/new` (mcpServers), но сервер самодостаточен
//! — его можно подключить к любому MCP-клиенту (`sql-kai mcp install`).
//!
//! Алиас необязателен. С алиасом сервер закреплён за одной базой (эту форму
//! ломать нельзя — её строит GUI). Без алиаса он обслуживает все профили
//! сразу: появляется tool `profiles`, а у остальных tools — параметр
//! `profile`. Так в конфиге MCP-клиента хватает одной записи на все базы.
//!
//! Все запросы идут через сервер сессий (GUI-брокер или holder): vault уже
//! разблокирован, туннели подняты, сессия read-only по умолчанию. stdout —
//! канал протокола (newline-delimited JSON-RPC), диагностика — в stderr.
//!
//! Ответ tool'а двухканальный: `content` с текстом (его читают клиенты,
//! которые не умеют structured output) и `structuredContent` по объявленной
//! `outputSchema`. Тексты tools/описания/ошибки — по-английски: их читает
//! модель, а не пользователь CLI.

use std::io::Read;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use serde_json::{json, Value};
use sql_kai_lib::db::{self, ExecResult, StatementResult};
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, HistoryEntry, Profile};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::cmd::introspect::split_table;
use crate::cmd::mcp_setup::{self, InstallArgs, StatusArgs};
use crate::cmd::schema as schema_cmd;
use crate::{broker_client, redact, session};

/// Версия MCP, которую сервер заявляет, если клиентскую не знает.
const PROTOCOL_FALLBACK: &str = "2025-06-18";
/// Строк на запрос по умолчанию — меньше, чем в CLI: ответ читает модель.
const DEFAULT_MAX_ROWS: usize = 200;

#[derive(Args)]
// Позиционный алиас и под-подкоманды (install/status) взаимоисключающи:
// без этого clap считает `mcp install` неоднозначным.
#[command(args_conflicts_with_subcommands = true)]
pub struct McpArgs {
    /// Профиль: имя, id или группа. Без него — сервер по всем профилям
    /// (tool `profiles` + параметр `profile` у остальных tools)
    alias: Option<String>,
    #[command(subcommand)]
    cmd: Option<McpCmd>,
}

#[derive(Subcommand)]
pub enum McpCmd {
    /// Прописать sql-kai в конфиг MCP-клиента (claude-code, codex, cursor, …)
    Install(InstallArgs),
    /// Где sql-kai уже подключён как MCP-сервер
    Status(StatusArgs),
}

pub async fn run(a: McpArgs) -> Result<ExitCode, AppError> {
    match a.cmd {
        Some(McpCmd::Install(args)) => return mcp_setup::install(args),
        Some(McpCmd::Status(args)) => return mcp_setup::status(args),
        None => {}
    }
    // Дальше stdin — транспорт JSON-RPC, а не человек, даже если сервер
    // запустили руками в терминале: любой вопрос (прод-барьер, confirm) съел бы
    // кадр протокола и завис бы, ожидая ответа, которого не будет.
    crate::input::set_non_interactive();
    let pinned = match a.alias.as_deref() {
        Some(alias) => Some(session::resolve_profile(alias)?),
        None => None,
    };
    Server { pinned }.serve().await
}

/// Состояние сервера на всё время работы процесса: либо закреплённый профиль,
/// либо мультипрофильный режим (профиль выбирает каждый вызов tool'а).
struct Server {
    pinned: Option<Profile>,
}

impl Server {
    fn multi(&self) -> bool {
        self.pinned.is_none()
    }

    async fn serve(&self) -> Result<ExitCode, AppError> {
        match &self.pinned {
            Some(p) => eprintln!(
                "sql-kai mcp: профиль {} ({} @ {}:{})",
                p.name, p.database, p.host, p.port
            ),
            None => eprintln!(
                "sql-kai mcp: все профили ({}) — выбор через параметр profile",
                store::load_profiles().map(|p| p.len()).unwrap_or(0)
            ),
        }

        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();
        let mut stdout = tokio::io::stdout();
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let Some(reply) = self.handle_line(&line).await else {
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
    async fn handle_line(&self, line: &str) -> Option<Value> {
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
            "tools/list" => Ok(json!({ "tools": tool_definitions(self.multi()) })),
            "tools/call" => return Some(self.tools_call(&id, &params).await),
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

    /// tools/call: любые ошибки — внутрь result (isError), а не в error
    /// протокола, чтобы модель видела текст и могла отреагировать.
    async fn tools_call(&self, id: &Value, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let (out, is_error) = match self.dispatch_tool(name, &args).await {
            Ok(out) => (out, false),
            Err(message) => (ToolOutput::plain(message), true),
        };
        let mut result = json!({
            "content": [{ "type": "text", "text": out.text }],
            "isError": is_error,
        });
        if let Some(structured) = out.structured {
            result["structuredContent"] = structured;
        }
        json!({ "jsonrpc": "2.0", "id": id, "result": result })
    }

    /// Профиль для вызова tool'а: закреплённый (тогда `profile` обязан
    /// совпасть с ним) либо выбранный аргументом. В мультипрофильном режиме
    /// единственный профиль подставляется сам — параметр нужен, только когда
    /// выбирать есть из чего.
    fn target(&self, args: &Value) -> Result<Profile, String> {
        let requested = args
            .get("profile")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match (&self.pinned, requested) {
            (Some(p), None) => Ok(p.clone()),
            (Some(p), Some(alias)) => {
                let want = session::resolve_profile(alias).map_err(|e| e.to_string())?;
                if want.id == p.id {
                    Ok(want)
                } else {
                    Err(format!(
                        "this server is pinned to profile '{}'; profile='{alias}' is not available here",
                        p.name
                    ))
                }
            }
            (None, Some(alias)) => session::resolve_profile(alias).map_err(|e| e.to_string()),
            (None, None) => {
                let all = store::load_profiles().map_err(|e| e.to_string())?;
                match all.len() {
                    0 => Err("no profiles configured in sql-kai".to_string()),
                    1 => Ok(all.into_iter().next().expect("len == 1")),
                    // список профилей бывает длинным — в ошибку идёт образец,
                    // полный ответ у tool'а profiles
                    n => Err(format!(
                        "missing required argument: profile ({n} available: {}{}) — call the `profiles` tool for the full list",
                        all.iter().take(10).map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "),
                        if n > 10 { ", …" } else { "" }
                    )),
                }
            }
        }
    }

    async fn dispatch_tool(&self, name: &str, args: &Value) -> Result<ToolOutput, String> {
        let table_arg = || -> Result<(String, String), String> {
            let spec = args
                .get("table")
                .and_then(Value::as_str)
                .ok_or("missing required argument: table")?;
            Ok(split_table(spec))
        };
        match name {
            "profiles" => {
                if !self.multi() {
                    return Err("this server is pinned to a single profile".to_string());
                }
                profiles_output()
            }
            "query" => {
                let profile = self.target(args)?;
                let sql = sql_argument(args)?;
                let max_rows = args
                    .get("maxRows")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                    .unwrap_or(DEFAULT_MAX_ROWS);
                let write = args.get("write").and_then(Value::as_bool).unwrap_or(false);
                // Барьер прода — до брокера и до записи в историю: у MCP нет
                // TTY, поэтому спросить некого и разрешение может прийти только
                // из SQL_KAI_ALLOW_PROD_WRITE в конфиге MCP-клиента. Второй
                // (страховочный) чек-пойнт живёт в BrokerClient::query.
                if write {
                    session::guard_prod_write(&profile).map_err(|e| e.to_string())?;
                }
                let q = run_query(&profile, &sql, max_rows, write, true, true).await?;
                let structured = query_structured(&sql, &q);
                Ok(ToolOutput::new(q.text, structured))
            }
            "schema" => {
                let profile = self.target(args)?;
                let opts = db::SchemaOptions {
                    schema: args
                        .get("schema")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    internal: flag(args, "internal"),
                    definitions: flag(args, "definitions"),
                    comments: flag(args, "comments"),
                };
                schema_output(&profile, &opts).await
            }
            "tables" => {
                let profile = self.target(args)?;
                let counts = args.get("counts").and_then(Value::as_bool).unwrap_or(false);
                let sql = if counts { db::TABLES_COUNTS_SQL } else { db::TABLES_SQL };
                let q = run_query(&profile, sql, 10_000, false, false, false).await?;
                let structured = rows_structured("tables", &q.exec, &[
                    "schema", "name", "kind",
                ], counts.then_some("approx_rows"));
                Ok(ToolOutput::new(q.text, structured))
            }
            "columns" => {
                let profile = self.target(args)?;
                let (schema, table) = table_arg()?;
                let sql = db::columns_sql(&db::regclass_literal(&schema, &table));
                let q = run_query(&profile, &sql, 10_000, false, false, false).await?;
                let structured = rows_structured(
                    "columns",
                    &q.exec,
                    &["name", "type", "nullable", "primary_key", "default", "comment"],
                    None,
                );
                Ok(ToolOutput::new(q.text, structured))
            }
            "indexes" => {
                let profile = self.target(args)?;
                let (schema, table) = table_arg()?;
                let sql = db::indexes_sql(&db::regclass_literal(&schema, &table));
                let q = run_query(&profile, &sql, 10_000, false, false, false).await?;
                let structured = rows_structured(
                    "indexes",
                    &q.exec,
                    &["name", "unique", "primary", "columns", "definition"],
                    None,
                );
                Ok(ToolOutput::new(q.text, structured))
            }
            "ddl" => {
                let profile = self.target(args)?;
                let (schema, table) = table_arg()?;
                let mut b = connect_broker().await?;
                let ddl = b
                    .ddl(&profile.id, &schema, &table)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(ToolOutput::new(ddl.clone(), json!({ "ddl": ddl })))
            }
            "open_table" => {
                let profile = self.target(args)?;
                let (schema, table) = table_arg()?;
                let mut b = connect_broker().await?;
                b.open_table(&profile.id, &schema, &table)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(ToolOutput::ok(format!(
                    "opened {schema}.{table} in a sql-kai tab"
                )))
            }
            "open_query" => {
                let profile = self.target(args)?;
                let sql = args
                    .get("sql")
                    .and_then(Value::as_str)
                    .ok_or("missing required argument: sql")?;
                let mut b = connect_broker().await?;
                b.open_query(&profile.id, sql)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(ToolOutput::ok(
                    "opened a query tab in sql-kai (not executed)".to_string(),
                ))
            }
            "selection" => {
                let profile = match self.target(args) {
                    Ok(p) => p,
                    // профиль не назвали: вкладка, на которую смотрит
                    // пользователь, почти наверняка принадлежит базе, к
                    // которой GUI подключался последней
                    Err(e) if args.get("profile").is_none() => last_gui_profile().ok_or(e)?,
                    Err(e) => return Err(e),
                };
                let mut b = connect_broker().await?;
                let text = b
                    .gui_selection(&profile.id)
                    .await
                    .map_err(|e| e.to_string())?;
                // Форму payload'а задаёт GUI, поэтому outputSchema у tool'а нет
                // и structuredContent мы не выдумываем — только текст.
                Ok(ToolOutput::plain(text))
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }
}

/// Ответ tool'а: текст (обратная совместимость) + structuredContent, если для
/// tool'а объявлена outputSchema.
struct ToolOutput {
    text: String,
    structured: Option<Value>,
}

impl ToolOutput {
    fn plain(text: String) -> Self {
        ToolOutput { text, structured: None }
    }

    fn new(text: String, structured: Value) -> Self {
        ToolOutput { text, structured: Some(structured) }
    }

    /// Ответ GUI-tool'ов: одно и то же сообщение в оба канала.
    fn ok(message: String) -> Self {
        let structured = json!({ "ok": true, "message": message });
        ToolOutput::new(message, structured)
    }
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

/// Хинты поведения tool'а (спецификация MCP): по ним клиент решает, спрашивать
/// ли подтверждение. `query` объявлен деструктивным — он умеет `write`;
/// остальные tools базу не меняют.
fn annotations(title: &str, read_only: bool, destructive: bool, idempotent: bool) -> Value {
    json!({
        "title": title,
        "readOnlyHint": read_only,
        "destructiveHint": destructive,
        "idempotentHint": idempotent,
        // база — закрытый, заранее известный домен, а не внешний мир
        "openWorldHint": false,
    })
}

/// Схема массива объектов с фиксированными строковыми полями — общая форма
/// structuredContent у интроспекции.
fn rows_output_schema(key: &str, fields: &[&str]) -> Value {
    let props: serde_json::Map<String, Value> = fields
        .iter()
        .map(|f| ((*f).to_string(), json!({ "type": ["string", "null"] })))
        .collect();
    json!({
        "type": "object",
        "properties": {
            key: {
                "type": "array",
                "items": { "type": "object", "properties": props },
            }
        },
        "required": [key],
    })
}

fn query_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "result_sets": {
                "type": "array",
                "description": "One entry per statement, in order",
                "items": {
                    "type": "object",
                    "properties": {
                        "columns": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" },
                                    "type": { "type": "string", "description": "PostgreSQL type name" },
                                },
                                "required": ["name", "type"],
                            },
                        },
                        "rows": {
                            "type": "array",
                            "description": "Row-major values; every value is text (NULL is null)",
                            "items": { "type": "array", "items": { "type": ["string", "null"] } },
                        },
                        "rows_affected": { "type": ["integer", "null"] },
                        "command_tag": { "type": ["string", "null"] },
                        "truncated": { "type": "boolean", "description": "Row cap hit: more rows exist" },
                    },
                    "required": ["columns", "rows"],
                },
            },
            "execution_time": { "type": "integer", "description": "Server round-trip in milliseconds" },
            "masked_columns": {
                "type": "array",
                "description": "Columns whose values were replaced with [redacted]",
                "items": { "type": "string" },
            },
        },
        "required": ["result_sets", "execution_time"],
    })
}

/// outputSchema tool'а `schema` — дерево `cmd::schema::SchemaDump` (camelCase).
fn schema_output_schema() -> Value {
    let obj = |props: Value, required: Value| json!({
        "type": "object", "properties": props, "required": required,
    });
    let str_or_null = json!({ "type": ["string", "null"] });
    let relation = obj(
        json!({
            "schema": { "type": "string" },
            "name": { "type": "string" },
            "kind": {
                "type": "string",
                "enum": ["table", "partitioned", "view", "matview", "foreign"],
            },
            "comment": str_or_null,
            "partitionKey": str_or_null,
            "partitionOf": str_or_null,
            "partitions": { "type": ["integer", "null"], "description": "Leaf partitions of a partitioned table (hidden unless `internal`)" },
            "rowSecurity": { "type": "boolean" },
            "definition": { "type": ["string", "null"], "description": "View body; only with `definitions`" },
            "columns": { "type": "array", "items": obj(json!({
                "name": { "type": "string" },
                "type": { "type": "string" },
                "nullable": { "type": "boolean" },
                "default": str_or_null,
                "identity": str_or_null,
                "generated": str_or_null,
                "comment": str_or_null,
            }), json!(["name", "type", "nullable"])) },
            "constraints": { "type": "array", "items": obj(json!({
                "name": { "type": "string" },
                "kind": {
                    "type": "string",
                    "enum": ["primary key", "unique", "foreign key", "exclude", "check"],
                },
                "definition": { "type": "string" },
            }), json!(["name", "kind", "definition"])) },
            "indexes": { "type": "array", "items": obj(json!({
                "name": { "type": "string" },
                "unique": { "type": "boolean" },
                "definition": { "type": "string" },
            }), json!(["name", "unique", "definition"])) },
            "triggers": { "type": "array", "items": obj(json!({
                "name": { "type": "string" },
                "timing": { "type": "string" },
                "events": { "type": "string" },
                "function": { "type": "string" },
                "enabled": { "type": "boolean" },
            }), json!(["name", "timing", "events", "function", "enabled"])) },
        }),
        json!(["schema", "name", "kind", "columns"]),
    );
    // Форма отношения расписана один раз, у `tables`: развёрнутая четырежды,
    // она весит больше, чем весь остальной tools/list. Остальные три массива
    // ссылаются на неё словами — `$ref` тут стоил бы совместимости с
    // клиентами, которые не резолвят ссылки в outputSchema.
    let same_as_tables = |what: &str| json!({
        "type": "array",
        "description": format!("{what}; same object shape as `tables`"),
        "items": { "type": "object" },
    });
    json!({
        "type": "object",
        "properties": {
            "database": { "type": "string" },
            "serverVersion": { "type": "string" },
            "schemas": { "type": "array", "items": obj(
                json!({
                    "name": { "type": "string" },
                    "tables": { "type": "array", "items": relation },
                    "views": same_as_tables("Views"),
                    "materializedViews": same_as_tables("Materialized views"),
                    "foreignTables": same_as_tables("Foreign tables"),
                    "enums": { "type": "array", "items": obj(json!({
                        "schema": { "type": "string" },
                        "name": { "type": "string" },
                        "values": { "type": "array", "items": { "type": "string" } },
                        "comment": str_or_null,
                    }), json!(["schema", "name", "values"])) },
                    "routines": { "type": "array", "items": obj(json!({
                        "schema": { "type": "string" },
                        "name": { "type": "string" },
                        "kind": { "type": "string", "enum": ["function", "procedure"] },
                        "arguments": { "type": "string" },
                        "returns": str_or_null,
                        "language": { "type": "string" },
                        "volatility": { "type": "string" },
                        "definition": { "type": ["string", "null"], "description": "Source; only with `definitions`" },
                        "comment": str_or_null,
                    }), json!(["schema", "name", "kind", "arguments", "language"])) },
                }),
                json!(["name"]),
            ) },
        },
        "required": ["database", "schemas"],
    })
}

fn tool_definitions(multi: bool) -> Value {
    let table_schema = json!({
        "type": "object",
        "properties": {
            "table": { "type": "string", "description": "Table as [schema.]name; schema defaults to public" }
        },
        "required": ["table"],
    });
    let gui_output_schema = json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" },
            "message": { "type": "string" },
        },
        "required": ["ok", "message"],
    });
    let mut tools = vec![
        json!({
            "name": "query",
            "description": "Run SQL in a PostgreSQL database of sql-kai. Unless `write` is set the batch runs inside a READ ONLY transaction — set `write` only when the user explicitly asked to modify data. Because of that transaction, a read call also refuses statements that would leave it or lift its read-only mode (COMMIT/ROLLBACK/END/ABORT/PREPARE TRANSACTION/DISCARD/SET TRANSACTION/SET …transaction_read_only); that refusal means the batch needs `write`, not a cleverer rewrite. Databases marked `production` (see the `profiles` tool) reject `write` from here no matter what: the user has to allow it out-of-band via SQL_KAI_ALLOW_PROD_WRITE in this server's environment, so do not retry a refusal — report it and let the user decide. Prefer `parameters` over string interpolation for user values. Sensitive-looking columns (password/secret/*_token/*_key) are masked in the output.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "SQL to execute; multiple ;-separated statements allowed" },
                    "file": { "type": "string", "description": "Path to a .sql file to execute instead of `sql` (migrations and other on-disk scripts). Must be a regular file with a .sql extension — this is not a way to read other files." },
                    "parameters": {
                        "type": "array",
                        "description": "Values for $1, $2 … placeholders; single-statement SQL only. Values are always sent as text — cast in SQL when a different type is needed ($1::uuid).",
                        "items": { "type": "string" },
                    },
                    "maxRows": { "type": "integer", "description": "Row cap per statement (default 200)" },
                    "write": { "type": "boolean", "description": "Allow writes for this call (default false)" }
                },
            },
            "outputSchema": query_output_schema(),
            "annotations": annotations("Run SQL", false, true, false),
        }),
        json!({
            "name": "schema",
            "description": "The whole schema of the database in ONE call: tables (including partitioned and foreign ones), views and materialized views with their columns, types, nullability, defaults, constraints, indexes and triggers, plus enum types, functions and procedures. Prefer this over walking tables → columns/ddl/indexes table by table: it is a single round-trip and one consistent catalog snapshot, at a cost that does not grow with the number of tables. System schemas, extension-owned objects (postgis, timescaledb), leaf partitions, view/function bodies and COMMENT ON texts are left out by default — the flags below bring them back.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "schema": { "type": "string", "description": "Restrict the dump to this schema (a system one is allowed, e.g. pg_catalog); default: every non-system schema" },
                    "definitions": { "type": "boolean", "description": "Include view bodies and function sources (default false)" },
                    "comments": { "type": "boolean", "description": "Include COMMENT ON texts of tables, columns, types and routines (default false)" },
                    "internal": { "type": "boolean", "description": "Include system schemas, extension-owned objects and leaf partitions (default false) — usually a lot of noise" }
                },
            },
            "outputSchema": schema_output_schema(),
            "annotations": annotations("Database schema", true, false, true),
        }),
        json!({
            "name": "tables",
            "description": "List tables, views and materialized views of the database — names only. For the actual structure prefer the `schema` tool, which returns everything in one call. `kind` is pg_class.relkind: r=table, v=view, m=materialized view, p=partitioned table, f=foreign table.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "counts": { "type": "boolean", "description": "Include approximate row counts (default false)" }
                },
            },
            "outputSchema": rows_output_schema("tables", &["schema", "name", "kind", "approx_rows"]),
            "annotations": annotations("List tables", true, false, true),
        }),
        json!({
            "name": "columns",
            "description": "Columns of a table: name, type, nullability, PK, default, comment.",
            "inputSchema": table_schema,
            "outputSchema": rows_output_schema(
                "columns",
                &["name", "type", "nullable", "primary_key", "default", "comment"],
            ),
            "annotations": annotations("Table columns", true, false, true),
        }),
        json!({
            "name": "ddl",
            "description": "CREATE TABLE / CREATE VIEW statement of a table or view.",
            "inputSchema": table_schema,
            "outputSchema": {
                "type": "object",
                "properties": { "ddl": { "type": "string" } },
                "required": ["ddl"],
            },
            "annotations": annotations("Table DDL", true, false, true),
        }),
        json!({
            "name": "indexes",
            "description": "Indexes of a table: name, uniqueness, columns, definition.",
            "inputSchema": table_schema,
            "outputSchema": rows_output_schema(
                "indexes",
                &["name", "unique", "primary", "columns", "definition"],
            ),
            "annotations": annotations("Table indexes", true, false, true),
        }),
        json!({
            "name": "open_table",
            "description": "Show a table to the user: opens the table as a tab in the sql-kai GUI. Use when the user asks to 'show'/'open' a table.",
            "inputSchema": table_schema,
            "outputSchema": gui_output_schema,
            "annotations": annotations("Open a table in the GUI", false, false, false),
        }),
        json!({
            "name": "open_query",
            "description": "Open a query tab in the sql-kai GUI pre-filled with the given SQL, WITHOUT executing it. Use to hand a prepared query over to the user.",
            "inputSchema": {
                "type": "object",
                "properties": { "sql": { "type": "string" } },
                "required": ["sql"],
            },
            "outputSchema": gui_output_schema,
            "annotations": annotations("Open a query in the GUI", false, false, false),
        }),
        json!({
            "name": "selection",
            "description": "What the user currently sees in the sql-kai GUI: the active tab (table with its filter/sort/page, or query with its SQL) and the rows, columns or cells they have selected — with the selected data. Use when the user refers to what's on their screen: 'this row', 'the selected rows', 'this column', 'что я выделил' etc. Without `profile` it reports on the database the user opened in the GUI most recently.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": annotations("Current GUI selection", true, false, false),
        }),
    ];
    if multi {
        for tool in &mut tools {
            tool["inputSchema"]["properties"]["profile"] = json!({
                "type": "string",
                "description": "Database to run against: profile name, id or group from the `profiles` tool. Optional only while sql-kai holds a single profile.",
            });
        }
        tools.insert(
            0,
            json!({
                "name": "profiles",
                "description": "List the PostgreSQL databases sql-kai is configured for. Every other tool takes the `profile` argument from here. Check `production` before writing anything.",
                "inputSchema": { "type": "object", "properties": {} },
                "outputSchema": {
                    "type": "object",
                    "properties": {
                        "profiles": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" },
                                    "id": { "type": "string" },
                                    "group": { "type": ["string", "null"] },
                                    "database": { "type": "string" },
                                    "host": { "type": "string" },
                                    "port": { "type": "integer" },
                                    "production": { "type": "boolean" },
                                },
                                "required": ["name", "id", "database", "production"],
                            },
                        }
                    },
                    "required": ["profiles"],
                },
                "annotations": annotations("List databases", true, false, true),
            }),
        );
    }
    Value::Array(tools)
}

async fn connect_broker() -> Result<broker_client::BrokerClient, String> {
    broker_client::connect_any().await.ok_or_else(|| {
        "нет сервера сессий: запусти приложение sql-kai (или настрой тихую \
         разблокировку vault: sql-kai vault trust)"
            .to_string()
    })
}

/// База, к которой GUI подключался последней, — дефолт GUI-tool'ов в
/// мультипрофильном режиме (у пользователя открыта именно её вкладка).
fn last_gui_profile() -> Option<Profile> {
    let marks = store::load_last_connected().ok()?;
    let (id, _) = marks
        .iter()
        .filter_map(|(id, m)| m.gui.map(|at| (id.clone(), at)))
        .max_by_key(|(_, at)| *at)?;
    store::load_profiles().ok()?.into_iter().find(|p| p.id == id)
}

/// Булев аргумент tool'а; отсутствие и `null` — false.
fn flag(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// Дамп всей схемы силами сервера сессий — тот же батч каталожных запросов,
/// что у `sql-kai schema` (см. [`crate::cmd::schema`]), и тот же рендер.
///
/// Ради этого tool всё и затевалось: без него модель делает `tables`, а затем
/// `columns`/`indexes`/`ddl` на каждую таблицу — десятки round-trip'ов и
/// каталог, собранный из разных моментов времени.
async fn schema_output(profile: &Profile, opts: &db::SchemaOptions) -> Result<ToolOutput, String> {
    // Версия сервера едет тем же round-trip'ом (в CLI она приходит из
    // `SHOW server_version` при открытии сессии, а у брокера сессия уже
    // открыта и наружу версию не отдаёт).
    let sql = format!("SHOW server_version;\n{}", db::schema_dump_sql(opts));
    let mut b = connect_broker().await?;
    let res = b
        .query(&profile.id, &sql, schema_cmd::MAX_ROWS, false, false)
        .await
        .map_err(|e| e.to_string())?;
    let mut results = res.exec.results;
    if results.is_empty() {
        return Err("the catalog batch returned no result sets".to_string());
    }
    let server_version = results
        .remove(0)
        .rows
        .first()
        .and_then(|row| row.first().cloned())
        .flatten()
        .unwrap_or_default();
    schema_cmd::check_parts(&results).map_err(|e| e.to_string())?;
    let truncated = results.iter().any(|r| r.truncated);

    let dump = schema_cmd::build_dump(&profile.database, &server_version, &results);
    let mut text = schema_cmd::render_text(&dump, opts);
    if truncated {
        text.push_str("-- ВНИМАНИЕ: каталог обрезан по лимиту строк — дамп неполный, сузь вывод параметром schema\n");
    }
    let structured = serde_json::to_value(&dump).map_err(|e| e.to_string())?;
    Ok(ToolOutput::new(text, structured))
}

fn profiles_output() -> Result<ToolOutput, String> {
    let profiles = store::load_profiles().map_err(|e| e.to_string())?;
    let items: Vec<Value> = profiles
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "id": p.id,
                "group": p.group,
                "database": p.database,
                "host": p.host,
                "port": p.port,
                "production": p.production,
            })
        })
        .collect();
    let structured = json!({ "profiles": items });
    let text = serde_json::to_string(&structured).map_err(|e| e.to_string())?;
    Ok(ToolOutput::new(text, structured))
}

/// Предел на файл в `file`. Настоящие миграции — килобайты; ограничение не про
/// удобство, а про то, что путь называет модель.
const SQL_FILE_CAP: u64 = 4 * 1024 * 1024;

/// SQL из файла, названного моделью. Путь недоверенный, поэтому три условия:
///   * расширение `.sql` — иначе `file` превращается в чтение любого файла
///     пользователя: содержимое не-SQL уедет модели обратно фрагментами в
///     `syntax error at or near "…"`;
///   * обычный файл — fifo заблокировал бы весь stdio-цикл сервера навсегда,
///     `/dev/zero` съел бы память (stat, а не open: открытие fifo само висит);
///   * размер — плюс `take` при чтении, чтобы файл не подрос между stat и open.
fn read_sql_file(path: &str) -> Result<String, String> {
    let p = std::path::Path::new(path);
    if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sql")) {
        return Err(format!("`file` must point to a .sql file, got '{path}'"));
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("cannot read file '{path}': {e}"))?;
    if !meta.is_file() {
        return Err(format!("'{path}' is not a regular file"));
    }
    if meta.len() > SQL_FILE_CAP {
        return Err(format!(
            "'{path}' is {} bytes — over the {SQL_FILE_CAP}-byte limit for `file`",
            meta.len()
        ));
    }
    let f = std::fs::File::open(p).map_err(|e| format!("cannot read file '{path}': {e}"))?;
    let mut sql = String::new();
    f.take(SQL_FILE_CAP + 1)
        .read_to_string(&mut sql)
        .map_err(|e| format!("cannot read file '{path}': {e}"))?;
    if sql.len() as u64 > SQL_FILE_CAP {
        return Err(format!("'{path}' grew over the {SQL_FILE_CAP}-byte limit while reading"));
    }
    Ok(sql)
}

/// SQL, как его задал вызывающий: либо `sql`, либо `file` (ровно одно из
/// двух), плюс подстановка `parameters`. Файл нужен для основного сценария
/// «примени миграцию»: у агента нет способа передать серверу stdin.
fn sql_argument(args: &Value) -> Result<String, String> {
    let inline = args.get("sql").and_then(Value::as_str);
    let file = args.get("file").and_then(Value::as_str);
    let sql = match (inline, file) {
        (Some(_), Some(_)) => {
            return Err("pass either `sql` or `file`, not both".to_string());
        }
        (Some(sql), None) => sql.to_string(),
        (None, Some(path)) => read_sql_file(path)?,
        (None, None) => return Err("missing required argument: sql (or file)".to_string()),
    };

    let params: Vec<String> = match args.get("parameters") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => Ok(s.clone()),
                // числа/булевы модель шлёт постоянно — принимаем как текст,
                // NULL параметром не передать (нужен литерал в самом SQL)
                Value::Number(n) => Ok(n.to_string()),
                Value::Bool(b) => Ok(b.to_string()),
                other => Err(format!("parameters must be strings, got {other}")),
            })
            .collect::<Result<_, _>>()?,
        Some(other) => return Err(format!("`parameters` must be an array, got {other}")),
    };
    if params.is_empty() {
        return Ok(sql);
    }
    let statements = db::split_statements(&sql);
    match statements.len() {
        1 => bind_parameters(&sql, &params),
        0 => Err("no SQL statement to run".to_string()),
        n => Err(format!(
            "`parameters` are supported for a single statement only, got {n} — send them one call at a time"
        )),
    }
}

/// Подставляет `params` в плейсхолдеры `$1..$N`.
///
/// Сервер сессий принимает только текст SQL, bind-параметров протокола в нём
/// нет — поэтому значения экранируются `quote_literal` и вставляются как
/// литералы. Сканер повторяет разбор `db::split_statements`: внутри строк,
/// dollar-quoted блоков и комментариев `$1` остаётся текстом, а не
/// плейсхолдером.
fn bind_parameters(sql: &str, params: &[String]) -> Result<String, String> {
    let b = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + params.len() * 8);
    let mut used = vec![false; params.len()];
    let mut seg = 0; // начало ещё не скопированного куска
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
                // E'…' также понимает обратные слэши
                let estring = i > 0
                    && (b[i - 1] == b'e' || b[i - 1] == b'E')
                    && (i < 2 || !(b[i - 2].is_ascii_alphanumeric() || b[i - 2] == b'_'));
                i += 1;
                while i < b.len() {
                    if estring && b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == b'\'' {
                        if b.get(i + 1) == Some(&b'\'') {
                            i += 2; // '' — экранированная кавычка
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
                // $tag$ … $tag$ — тело копируем как есть; $N — плейсхолдер
                let mut j = i + 1;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                let is_tag =
                    j < b.len() && b[j] == b'$' && (j == i + 1 || !b[i + 1].is_ascii_digit());
                if is_tag {
                    let tag = &b[i..=j];
                    i = j + 1;
                    while i + tag.len() <= b.len() && &b[i..i + tag.len()] != tag {
                        i += 1;
                    }
                    i = (i + tag.len()).min(b.len());
                } else if b.get(i + 1).is_some_and(u8::is_ascii_digit) {
                    let mut j = i + 1;
                    while j < b.len() && b[j].is_ascii_digit() {
                        j += 1;
                    }
                    let n: usize = sql[i + 1..j]
                        .parse()
                        .map_err(|_| format!("placeholder ${} is out of range", &sql[i + 1..j]))?;
                    if n == 0 {
                        return Err("placeholder $0 is invalid: parameters start at $1".to_string());
                    }
                    let value = params.get(n - 1).ok_or_else(|| {
                        format!(
                            "SQL references ${n}, but only {} parameter(s) were supplied",
                            params.len()
                        )
                    })?;
                    out.push_str(&sql[seg..i]);
                    out.push_str(&db::quote_literal(value));
                    used[n - 1] = true;
                    i = j;
                    seg = i;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    if let Some(k) = used.iter().position(|u| !u) {
        return Err(format!(
            "parameter ${} is never referenced in the SQL",
            k + 1
        ));
    }
    out.push_str(&sql[seg..]);
    Ok(out)
}

/// Результат SQL из сервера сессий: текст (совместимость) плюс разобранный
/// exec и типы колонок для structuredContent.
struct QueryOutcome {
    text: String,
    exec: ExecResult,
    types: Vec<Option<Vec<(String, u32)>>>,
    masked: Vec<String>,
}

/// SQL через сервер сессий; текст ответа — компактный JSON (все значения
/// строками, как отдаёт simple-query протокол). `record` — писать в историю
/// (пользовательские запросы; интроспекция историю не засоряет),
/// `with_types` — спрашивать типы колонок (нужны только tool'у query).
async fn run_query(
    profile: &Profile,
    sql: &str,
    max_rows: usize,
    write: bool,
    record: bool,
    with_types: bool,
) -> Result<QueryOutcome, String> {
    let mut b = connect_broker().await?;
    let outcome = b
        .query(&profile.id, sql, max_rows.max(1), write, with_types)
        .await;
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
    let res = outcome.map_err(|e| e.to_string())?;
    let mut exec = res.exec;
    let masked = redact::redact_exec(&mut exec);
    let mut payload = serde_json::to_value(&exec).map_err(|e| e.to_string())?;
    if !masked.is_empty() {
        payload["maskedColumns"] = json!(masked);
    }
    let text = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    Ok(QueryOutcome {
        text,
        exec,
        types: res.column_types.unwrap_or_default(),
        masked,
    })
}

/// structuredContent tool'а query: результаты по стейтментам с именами и
/// типами колонок.
fn query_structured(sql: &str, q: &QueryOutcome) -> Value {
    let statements = db::split_statements(sql);
    let sets: Vec<Value> = q
        .exec
        .results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let columns: Vec<Value> = r
                .columns
                .iter()
                .enumerate()
                .map(|(k, name)| {
                    json!({ "name": name, "type": column_type(&q.types, i, k) })
                })
                .collect();
            json!({
                "columns": columns,
                "rows": r.rows,
                "rows_affected": r.rows_affected,
                "command_tag": command_tag(statements.get(i).map(String::as_str), r),
                "truncated": r.truncated,
            })
        })
        .collect();
    let mut out = json!({ "result_sets": sets, "execution_time": q.exec.duration_ms });
    if !q.masked.is_empty() {
        out["masked_columns"] = json!(q.masked);
    }
    out
}

/// Имя типа k-й колонки i-го стейтмента. Сервер сессий отдаёт oid; неизвестные
/// (доменные/составные) типы честно показываем как oid, а не выдаём за text.
fn column_type(types: &[Option<Vec<(String, u32)>>], stmt: usize, col: usize) -> String {
    let oid = types
        .get(stmt)
        .and_then(|c| c.as_ref())
        .and_then(|c| c.get(col))
        .map(|(_, oid)| *oid);
    match oid {
        Some(oid) => db::Type::from_oid(oid)
            .map(|t| t.name().to_string())
            .unwrap_or_else(|| format!("oid:{oid}")),
        // типы не запрашивались либо стейтмент не подготовился (DDL,
        // зависимость от предыдущего стейтмента) — значения всё равно текст
        None => "text".to_string(),
    }
}

/// Реконструкция CommandComplete-тега (`UPDATE 3`, `CREATE TABLE`): simple-query
/// путь отдаёт только число строк, поэтому глагол берём из самого стейтмента.
fn command_tag(stmt: Option<&str>, r: &StatementResult) -> Value {
    let Some(stmt) = stmt else {
        return Value::Null;
    };
    let mut words = stmt
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_uppercase);
    let Some(first) = words.next() else {
        return Value::Null;
    };
    // теги команд, которые Postgres дополняет числом строк
    const COUNTED: &[&str] = &[
        "SELECT", "UPDATE", "DELETE", "MERGE", "COPY", "FETCH", "MOVE",
    ];
    match r.rows_affected {
        Some(n) if first == "INSERT" => json!(format!("INSERT 0 {n}")),
        Some(n) if COUNTED.contains(&first.as_str()) => json!(format!("{first} {n}")),
        _ => match words.next() {
            Some(second) => json!(format!("{first} {second}")),
            None => json!(first),
        },
    }
}

/// structuredContent интроспекции: строки результата → массив объектов с
/// заданными именами полей. `extra` — поле последней колонки, которой может и
/// не быть (`tables --counts`).
fn rows_structured(key: &str, exec: &ExecResult, fields: &[&str], extra: Option<&str>) -> Value {
    let mut names: Vec<&str> = fields.to_vec();
    if let Some(e) = extra {
        names.push(e);
    }
    let items: Vec<Value> = exec
        .results
        .iter()
        .flat_map(|r| r.rows.iter())
        .map(|row| {
            let obj: serde_json::Map<String, Value> = names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let v = row
                        .get(i)
                        .cloned()
                        .flatten()
                        .map(Value::String)
                        .unwrap_or(Value::Null);
                    ((*name).to_string(), v)
                })
                .collect();
            Value::Object(obj)
        })
        .collect();
    json!({ key: items })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_and_escapes_parameters() {
        let sql = "SELECT * FROM t WHERE name = $1 AND age > $2";
        let out = bind_parameters(sql, &["O'Brien".into(), "18".into()]).unwrap();
        assert_eq!(out, "SELECT * FROM t WHERE name = 'O''Brien' AND age > '18'");
    }

    /// `file` — не способ прочитать произвольный файл: путь называет модель, а
    /// не-SQL вернулся бы ей фрагментами в тексте синтаксической ошибки. Плюс
    /// fifo (повесил бы stdio-цикл) и каталог отсекаются как не regular file.
    #[test]
    fn sql_file_argument_is_restricted() {
        let dir = std::env::temp_dir().join(format!("sql-kai-mcp-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = dir.join("m.sql");
        std::fs::write(&ok, "SELECT 1;").unwrap();
        assert_eq!(read_sql_file(ok.to_str().unwrap()).unwrap(), "SELECT 1;");

        let not_sql = dir.join("id_rsa");
        std::fs::write(&not_sql, "-----BEGIN PRIVATE KEY-----").unwrap();
        assert!(read_sql_file(not_sql.to_str().unwrap()).is_err());
        // каталог с нужным расширением — тоже не файл
        let as_dir = dir.join("d.sql");
        std::fs::create_dir_all(&as_dir).unwrap();
        assert!(read_sql_file(as_dir.to_str().unwrap()).is_err());
        assert!(read_sql_file(dir.join("нет.sql").to_str().unwrap()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Значение с обратным слэшем уходит в E'…': при
    /// `standard_conforming_strings = off` обычный литерал прочитал бы `\` как
    /// escape, и значение поехало бы (а `'…\'` — сломало бы запрос).
    #[test]
    fn binds_backslash_as_an_escape_string() {
        let out = bind_parameters("SELECT $1", &["C:\\tmp\\x".into()]).unwrap();
        assert_eq!(out, "SELECT E'C:\\\\tmp\\\\x'");
        let out = bind_parameters("SELECT $1", &["ends with \\".into()]).unwrap();
        assert_eq!(out, "SELECT E'ends with \\\\'");
    }

    #[test]
    fn leaves_placeholders_inside_literals_and_comments() {
        let sql = "SELECT '$1', \"$1\", $1 -- $1\n/* $1 */";
        let out = bind_parameters(sql, &["v".into()]).unwrap();
        assert_eq!(out, "SELECT '$1', \"$1\", 'v' -- $1\n/* $1 */");
    }

    #[test]
    fn leaves_dollar_quoted_bodies_alone() {
        let sql = "DO $do$ BEGIN RAISE NOTICE '$1'; END $do$";
        assert_eq!(bind_parameters(sql, &[]).unwrap(), sql);
    }

    #[test]
    fn rejects_missing_and_unused_parameters() {
        assert!(bind_parameters("SELECT $2", &["a".into()]).is_err());
        assert!(bind_parameters("SELECT $1", &["a".into(), "b".into()]).is_err());
        assert!(bind_parameters("SELECT $0", &["a".into()]).is_err());
    }

    #[test]
    fn parameters_require_a_single_statement() {
        let args = json!({ "sql": "SELECT $1; SELECT 2", "parameters": ["a"] });
        assert!(sql_argument(&args).is_err());
        let args = json!({ "sql": "SELECT $1", "parameters": ["a"] });
        assert_eq!(sql_argument(&args).unwrap(), "SELECT 'a'");
    }

    #[test]
    fn sql_and_file_are_mutually_exclusive() {
        assert!(sql_argument(&json!({ "sql": "SELECT 1", "file": "/tmp/x.sql" })).is_err());
        assert!(sql_argument(&json!({})).is_err());
    }

    /// `schema` — главный tool сервера: он должен быть в обоих режимах, быть
    /// read-only и объявлять все четыре фильтра дампа.
    #[test]
    fn schema_tool_is_declared_read_only_with_its_filters() {
        for multi in [false, true] {
            let tools = tool_definitions(multi);
            let t = tools
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == "schema")
                .expect("tool schema");
            assert_eq!(t["annotations"]["readOnlyHint"], true);
            assert_eq!(t["annotations"]["destructiveHint"], false);
            let props = &t["inputSchema"]["properties"];
            for f in ["schema", "definitions", "comments", "internal"] {
                assert!(props[f].is_object(), "{f} missing (multi={multi})");
            }
            // профиль появляется только в мультирежиме — как у остальных tools
            assert_eq!(props.get("profile").is_some(), multi);
            assert!(t["outputSchema"]["properties"]["schemas"].is_object());
        }
    }

    #[test]
    fn multi_mode_adds_profile_argument_and_tool() {
        let pinned = tool_definitions(false);
        assert!(pinned[0]["inputSchema"]["properties"]
            .get("profile")
            .is_none());
        assert!(pinned.as_array().unwrap().iter().all(|t| t["name"] != "profiles"));
        let multi = tool_definitions(true);
        assert_eq!(multi[0]["name"], "profiles");
        let query = multi
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "query")
            .unwrap();
        assert!(query["inputSchema"]["properties"]["profile"].is_object());
        // аннотации и outputSchema — у каждого tool'а
        for tool in multi.as_array().unwrap() {
            assert!(tool["annotations"]["title"].is_string(), "{}", tool["name"]);
        }
    }

    #[test]
    fn query_structured_carries_types_nulls_and_tags() {
        let exec = ExecResult {
            results: vec![StatementResult {
                columns: vec!["id".to_string(), "name".to_string()],
                rows: vec![vec![Some("1".to_string()), None]],
                rows_affected: Some(1),
                truncated: false,
            }],
            duration_ms: 7,
        };
        let q = QueryOutcome {
            text: String::new(),
            exec,
            // oid 23 = int4, 25 = text
            types: vec![Some(vec![("id".to_string(), 23), ("name".to_string(), 25)])],
            masked: vec!["name".to_string()],
        };
        let v = query_structured("SELECT id, name FROM t", &q);
        assert_eq!(v["result_sets"][0]["columns"][0]["type"], "int4");
        assert_eq!(v["result_sets"][0]["columns"][1]["type"], "text");
        assert_eq!(v["result_sets"][0]["rows"][0][0], "1");
        assert_eq!(v["result_sets"][0]["rows"][0][1], Value::Null);
        assert_eq!(v["result_sets"][0]["command_tag"], "SELECT 1");
        assert_eq!(v["result_sets"][0]["truncated"], false);
        assert_eq!(v["execution_time"], 7);
        assert_eq!(v["masked_columns"][0], "name");
    }

    #[test]
    fn rows_structured_maps_columns_by_position() {
        let exec = ExecResult {
            results: vec![StatementResult {
                columns: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                rows: vec![vec![
                    Some("public".to_string()),
                    Some("users".to_string()),
                    Some("r".to_string()),
                ]],
                rows_affected: Some(1),
                truncated: false,
            }],
            duration_ms: 1,
        };
        let v = rows_structured("tables", &exec, &["schema", "name", "kind"], None);
        assert_eq!(v["tables"][0]["schema"], "public");
        assert_eq!(v["tables"][0]["kind"], "r");
        // при --counts появляется четвёртое поле, которого в строке нет
        let v = rows_structured("tables", &exec, &["schema", "name", "kind"], Some("approx_rows"));
        assert_eq!(v["tables"][0]["approx_rows"], Value::Null);
    }

    #[test]
    fn command_tag_reconstructs_postgres_tags() {
        let r = StatementResult { rows_affected: Some(3), ..Default::default() };
        assert_eq!(command_tag(Some("UPDATE t SET a = 1"), &r), json!("UPDATE 3"));
        assert_eq!(command_tag(Some("INSERT INTO t VALUES (1)"), &r), json!("INSERT 0 3"));
        assert_eq!(command_tag(Some("CREATE TABLE t ()"), &r), json!("CREATE TABLE"));
        assert_eq!(command_tag(None, &r), Value::Null);
    }
}
