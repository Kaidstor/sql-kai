//! Running SQL and getting data out: the editor's execute, the full-result
//! export, table-tab pages and the grid's save-to-file helpers.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::{self, ExecResult, StatementResult, TxStatus};
use crate::error::AppError;

use super::session::{client_and_tx, session_is_production};
use super::AppState;

/// Прод-барьер GUI-сессии: без явного write-intent батч на production-профиле
/// идёт внутри `BEGIN READ ONLY` — что считается записью, решает Postgres, а
/// не regex на фронте (тот пропускал CREATE, CALL, DO, пишущие функции).
/// Отказ уходит на фронт кодом "read_only"; тот показывает подтверждение и
/// повторяет запрос с `prod_write=true`.
///
/// Открытая транзакция (status != Idle) исполняется без обёртки: на
/// prod-сессии она могла открыться только из батча с подтверждённым
/// write-intent — без него `execute_read_only` всегда закрывает свой блок и
/// оставляет Idle (пользовательский BEGIN внутри блока — предупреждение-no-op).
fn prod_read_only_guard(
    state: &State<'_, AppState>,
    session_id: &str,
    status: TxStatus,
    prod_write: Option<bool>,
) -> Result<bool, AppError> {
    Ok(status == TxStatus::Idle
        && !prod_write.unwrap_or(false)
        && session_is_production(state, session_id)?)
}

// Аргументы — форма IPC-вызова из фронтенда, а не сигнатура для рук.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn execute_sql(
    state: State<'_, AppState>,
    session_id: String,
    sql: String,
    max_rows: Option<usize>,
    auto_begin: Option<bool>,
    parameters: Option<Vec<String>>,
    prod_write: Option<bool>,
) -> Result<ExecResult, AppError> {
    let (client, tx) = client_and_tx(&state, &session_id)?;
    // Сессия выполняет батчи simple-query, где bind-параметров нет вовсе —
    // значения уходят литералами.
    let sql = match parameters.as_deref() {
        Some(params) if !params.is_empty() => db::bind_parameters(&sql, params)?,
        _ => sql,
    };
    let executor = db::QueryExecutor::new(&client, &tx);
    let max_rows = max_rows.unwrap_or(1000).clamp(1, 100_000);
    if prod_read_only_guard(&state, &session_id, executor.status(), prod_write)? {
        // auto_begin не префиксуется: блок закрывается сам, а manual-commit
        // транзакция на проде начинается только с подтверждённого батча.
        return executor.execute_read_only(&sql, max_rows).await;
    }
    // Manual-commit mode: hold a transaction open across runs by opening one
    // when the connection is idle, so the user never has to type BEGIN.
    let prepended = auto_begin.unwrap_or(false) && executor.status() == TxStatus::Idle;
    let sql = if prepended {
        format!("BEGIN;\n{sql}")
    } else {
        sql
    };
    let mut result = executor.execute(&sql, max_rows).await;
    // Hide the synthetic BEGIN's result: the frontend numbers result blocks by
    // the statements of the SQL it sent (per-statement export relies on it),
    // and an "OK" block for a BEGIN the user never typed is just noise.
    if prepended {
        if let Ok(r) = &mut result {
            if !r.results.is_empty() {
                r.results.remove(0);
            }
        }
    }
    result
}

/// Full-result export: re-runs `sql` and streams the rows of statement
/// `statement_index` into `path` (csv/json/md/xlsx) with no row limit — the
/// grid's fetch cap does not apply here. The rows stream into a sibling
/// `.part` file that replaces `path` only on success, so a failed export
/// neither leaves a half-written dump nor destroys a pre-existing file it
/// never wrote (XLSX only touches the disk in its final save).
// Аргументы — форма IPC-вызова из фронтенда, а не сигнатура для рук.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn export_sql(
    state: State<'_, AppState>,
    session_id: String,
    sql: String,
    statement_index: Option<usize>,
    format: String,
    path: String,
    auto_begin: Option<bool>,
    parameters: Option<Vec<String>>,
    prod_write: Option<bool>,
) -> Result<db::ExportResult, AppError> {
    let format = db::ExportFormat::parse(&format)?;
    let sql = match parameters.as_deref() {
        Some(params) if !params.is_empty() => db::bind_parameters(&sql, params)?,
        _ => sql,
    };
    let (client, tx) = client_and_tx(&state, &session_id)?;
    let executor = db::QueryExecutor::new(&client, &tx);
    let before = executor.status();
    let read_only = prod_read_only_guard(&state, &session_id, before, prod_write)?;
    // Mirror execute_sql's manual-commit wrapping — the re-run must not
    // autocommit a write that Run would have kept inside the open transaction.
    // The prepended BEGIN emits its own result set, shifting the numbering.
    // Внутри read-only блока BEGIN не префиксуется (см. execute_sql).
    let (sql, statement_index) =
        if !read_only && auto_begin.unwrap_or(false) && before == TxStatus::Idle {
            (format!("BEGIN;\n{sql}"), statement_index.unwrap_or(0) + 1)
        } else {
            (sql, statement_index.unwrap_or(0))
        };
    let tmp = format!("{path}.part");
    let result = if read_only {
        // Тот же блок, что у execute_read_only, только вокруг стримящего
        // экспорта: гейт-чеки, BEGIN READ ONLY, COMMIT/ROLLBACK.
        match db::begin_read_only(&client, &sql).await {
            Ok(()) => {
                let r = db::export_statement(&client, &sql, statement_index, format, &tmp).await;
                let sql_ok = !matches!(&r, Err(db::ExportError::Sql(_)));
                match (r, db::end_read_only(&client, sql_ok).await) {
                    (Ok(out), Ok(())) => Ok(out),
                    // Экспорт удался, но блок не закрылся — следующий вызов
                    // унаследовал бы чужую транзакцию; наружу как SQL-ошибка.
                    (Ok(_), Err(e)) => Err(db::ExportError::Sql(e)),
                    (Err(e), _) => Err(e),
                }
            }
            Err(e) => Err(db::ExportError::Sql(e)),
        }
    } else {
        db::export_statement(&client, &sql, statement_index, format, &tmp).await
    };
    if read_only {
        // Блок открыт и закрыт здесь же — соединение вне транзакции, что бы
        // ни было в батче (как QueryExecutor::execute_read_only).
        executor.mark_idle();
    } else {
        // The re-run goes through the same connection — keep the tx badge
        // honest. Only a database error can have aborted the transaction;
        // local failures (unwritable path, XLSX cap) never touched the
        // server, so they count ok.
        let sql_ok = !matches!(&result, Err(db::ExportError::Sql(_)));
        executor.advance(before, &sql, sql_ok);
    }
    match result {
        Ok(outcome) => {
            std::fs::rename(&tmp, &path)?;
            Ok(outcome)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TablePageResult {
    pub result: StatementResult,
    pub duration_ms: u64,
    pub approx_rows: i64,
}

/// One ORDER BY entry; the grid sends a list for multi-column sort.
#[derive(Deserialize)]
pub struct SortSpec {
    pub column: String,
    pub dir: Option<String>,
}

// Args mirror the frontend IPC call (schema/table/paging/sort/filter) — a Tauri
// command's parameters are its wire contract, so grouping them into a struct
// here would just move the arg list into the JSON payload.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn get_table_page(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
    limit: u32,
    offset: u64,
    sorts: Option<Vec<SortSpec>>,
    filter: Option<String>,
) -> Result<TablePageResult, AppError> {
    let (client, tx) = client_and_tx(&state, &session_id)?;
    let executor = db::QueryExecutor::new(&client, &tx);
    // Просмотр таблицы — заведомо чтение, write-intent у него не бывает; но
    // `filter` — сырой SQL, и через `;` в нём батч дописывается вторым
    // стейтментом. На проде обе строки с фильтром идут в read-only блоке.
    let read_only = prod_read_only_guard(&state, &session_id, executor.status(), None)?;
    let qualified = format!("{}.{}", db::quote_ident(&schema), db::quote_ident(&table));
    let limit = limit.clamp(1, 1000);

    // User-editable WHERE expression (FK navigation, filter bar) — raw SQL by
    // design, like the query editor itself.
    let where_clause = filter
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(|f| format!(" WHERE {f}"));

    let mut sql = format!("SELECT * FROM {qualified}");
    if let Some(w) = &where_clause {
        sql.push_str(w);
    }
    let order: Vec<String> = sorts
        .unwrap_or_default()
        .iter()
        .filter(|s| !s.column.is_empty())
        .map(|s| {
            let dir = match s.dir.as_deref() {
                Some("desc") | Some("DESC") => "DESC",
                _ => "ASC",
            };
            format!("{} {dir}", db::quote_ident(&s.column))
        })
        .collect();
    if !order.is_empty() {
        sql.push_str(&format!(" ORDER BY {}", order.join(", ")));
    }
    sql.push_str(&format!(" LIMIT {limit} OFFSET {offset}"));

    let exec = if read_only {
        executor.execute_read_only(&sql, limit as usize).await?
    } else {
        db::execute(&client, &sql, limit as usize).await?
    };
    let result = exec.results.into_iter().next().unwrap_or_default();

    let approx_rows = if let Some(w) = &where_clause {
        // Planner row estimate for the filtered set — cheap, unlike count(*).
        let explain = format!("EXPLAIN (FORMAT JSON) SELECT * FROM {qualified}{w}");
        let explained = if read_only {
            executor.execute_read_only(&explain, 10).await
        } else {
            db::execute(&client, &explain, 10).await
        };
        match explained {
            Ok(r) => r
                .results
                .first()
                .and_then(|res| res.rows.first())
                .and_then(|row| row[0].as_deref())
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .and_then(|v| v.get(0)?.get("Plan")?.get("Plan Rows")?.as_i64())
                .unwrap_or(-1),
            Err(_) => -1,
        }
    } else {
        let approx_sql = format!(
            "SELECT reltuples::bigint FROM pg_class WHERE oid = {}::regclass",
            db::quote_literal(&qualified)
        );
        db::query_scalar(&client, &approx_sql)
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(-1)
    };

    Ok(TablePageResult {
        result,
        duration_ms: exec.duration_ms,
        approx_rows,
    })
}

/// Writes an exported result set to the path picked in the save dialog.
#[tauri::command]
pub fn save_text_file(path: String, contents: String) -> Result<(), AppError> {
    std::fs::write(&path, contents)?;
    Ok(())
}

/// Writes the grid's current selection (already materialized on the client,
/// staged edits included) to `path` as XLSX. The binary counterpart of
/// [`save_text_file`] — the grid builds CSV/JSON text itself, but XLSX needs
/// the workbook writer, so the rows come over verbatim.
#[tauri::command]
pub fn save_rows_xlsx(
    path: String,
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
) -> Result<(), AppError> {
    db::write_rows_xlsx(&path, &columns, &rows)?;
    Ok(())
}
