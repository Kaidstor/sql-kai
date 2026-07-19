//! pg_catalog introspection for the sidebar, structure tab, autocomplete and
//! filter suggestions. Thin projections of the SQL in db::catalog.

use serde::Serialize;
use tauri::State;

use crate::db::{self, cell, cell_bool};
use crate::error::AppError;

use super::session::client_of;
use super::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub schema: String,
    pub name: String,
    pub kind: String,
}

#[tauri::command]
pub async fn list_tables(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<TableInfo>, AppError> {
    let client = client_of(&state, &session_id)?;
    let exec = db::execute(&client, db::TABLES_SQL, 100_000).await?;
    let mut out = Vec::new();
    for row in exec.results.iter().flat_map(|r| r.rows.iter()) {
        let kind = match row.get(2).and_then(|v| v.as_deref()) {
            Some("v") => "view",
            Some("m") => "matview",
            Some("f") => "foreign",
            _ => "table",
        };
        out.push(TableInfo {
            schema: cell(row, 0),
            name: cell(row, 1),
            kind: kind.to_string(),
        });
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableColumns {
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
}

/// Column names for every user relation in one round-trip — feeds the SQL
/// editor's schema autocomplete, so only names are selected (no types/PKs).
const ALL_COLUMNS_SQL: &str = "\
SELECT n.nspname, c.relname, a.attname
FROM pg_attribute a
JOIN pg_class c ON c.oid = a.attrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r','p','v','m','f')
  AND a.attnum > 0 AND NOT a.attisdropped
  AND n.nspname NOT IN ('pg_catalog','information_schema')
  AND n.nspname NOT LIKE 'pg_toast%'
  AND n.nspname NOT LIKE 'pg_temp%'
ORDER BY n.nspname, c.relname, a.attnum";

#[tauri::command]
pub async fn list_all_columns(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<TableColumns>, AppError> {
    let client = client_of(&state, &session_id)?;
    let exec = db::execute(&client, ALL_COLUMNS_SQL, 500_000).await?;
    let mut out: Vec<TableColumns> = Vec::new();
    // Rows arrive ordered by schema+table, so grouping consecutively works.
    for row in exec.results.iter().flat_map(|r| r.rows.iter()) {
        let schema = cell(row, 0);
        let table = cell(row, 1);
        let column = cell(row, 2);
        match out.last_mut() {
            Some(last) if last.schema == schema && last.table == table => {
                last.columns.push(column);
            }
            _ => out.push(TableColumns {
                schema,
                table,
                columns: vec![column],
            }),
        }
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub is_pk: bool,
    pub default_expr: Option<String>,
    pub comment: Option<String>,
}

async fn introspect_rows(
    state: &State<'_, AppState>,
    session_id: &str,
    sql: &str,
) -> Result<Vec<Vec<Option<String>>>, AppError> {
    let client = client_of(state, session_id)?;
    db::query_rows(&client, sql).await
}

#[tauri::command]
pub async fn list_columns(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<Vec<ColumnInfo>, AppError> {
    let sql = db::columns_sql(&db::regclass_literal(&schema, &table));
    let rows = introspect_rows(&state, &session_id, &sql).await?;
    Ok(rows
        .into_iter()
        .map(|row| ColumnInfo {
            name: cell(&row, 0),
            data_type: cell(&row, 1),
            nullable: cell_bool(&row, 2),
            is_pk: cell_bool(&row, 3),
            default_expr: row[4].clone(),
            comment: row[5].clone(),
        })
        .collect())
}

/// See db::table_ddl — assembled from the catalogs (no SHOW CREATE TABLE in PG).
#[tauri::command]
pub async fn get_table_ddl(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<String, AppError> {
    let client = client_of(&state, &session_id)?;
    db::table_ddl(&client, &schema, &table).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub primary: bool,
    pub columns: Option<String>,
    pub definition: String,
}

#[tauri::command]
pub async fn list_indexes(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<Vec<IndexInfo>, AppError> {
    let sql = db::indexes_sql(&db::regclass_literal(&schema, &table));
    let rows = introspect_rows(&state, &session_id, &sql).await?;
    Ok(rows
        .into_iter()
        .map(|row| IndexInfo {
            name: cell(&row, 0),
            unique: cell_bool(&row, 1),
            primary: cell_bool(&row, 2),
            columns: row[3].clone(),
            definition: cell(&row, 4),
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationInfo {
    pub name: String,
    pub columns: Option<String>,
    pub ref_table: String,
    pub ref_columns: Option<String>,
    pub on_update: String,
    pub on_delete: String,
}

#[tauri::command]
pub async fn list_relations(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<Vec<RelationInfo>, AppError> {
    let sql = db::relations_sql(&db::regclass_literal(&schema, &table));
    let rows = introspect_rows(&state, &session_id, &sql).await?;
    Ok(rows
        .into_iter()
        .map(|row| RelationInfo {
            name: cell(&row, 0),
            columns: row[1].clone(),
            ref_table: cell(&row, 2),
            ref_columns: row[3].clone(),
            on_update: cell(&row, 4),
            on_delete: cell(&row, 5),
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerInfo {
    pub name: String,
    pub timing: String,
    pub events: String,
    pub definition: String,
    pub enabled: bool,
}

#[tauri::command]
pub async fn list_triggers(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<Vec<TriggerInfo>, AppError> {
    let sql = db::triggers_sql(&db::regclass_literal(&schema, &table));
    let rows = introspect_rows(&state, &session_id, &sql).await?;
    Ok(rows
        .into_iter()
        .map(|row| TriggerInfo {
            name: cell(&row, 0),
            timing: cell(&row, 1),
            events: cell(&row, 2),
            definition: cell(&row, 3),
            enabled: cell_bool(&row, 4),
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnumTypeInfo {
    pub schema: String,
    pub name: String,
    pub labels: Vec<String>,
}

/// Every enum type of the database with its labels — feeds filter-value
/// suggestions; enums rarely change, the frontend caches per profile.
#[tauri::command]
pub async fn list_enums(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<EnumTypeInfo>, AppError> {
    let rows = introspect_rows(&state, &session_id, db::ENUMS_SQL).await?;
    let mut out: Vec<EnumTypeInfo> = Vec::new();
    // Rows arrive ordered by schema+type, so grouping consecutively works.
    for row in rows {
        let schema = cell(&row, 0);
        let name = cell(&row, 1);
        let label = cell(&row, 2);
        match out.last_mut() {
            Some(last) if last.schema == schema && last.name == name => {
                last.labels.push(label);
            }
            _ => out.push(EnumTypeInfo {
                schema,
                name,
                labels: vec![label],
            }),
        }
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyInfo {
    pub name: String,
    pub command: String,
    pub permissive: bool,
    /// NULL = PUBLIC (pg_policy stores roles={0} for it).
    pub roles: Option<String>,
    pub using_expr: Option<String>,
    pub check_expr: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TablePolicies {
    pub rls_enabled: bool,
    pub rls_forced: bool,
    pub policies: Vec<PolicyInfo>,
}

#[tauri::command]
pub async fn list_policies(
    state: State<'_, AppState>,
    session_id: String,
    schema: String,
    table: String,
) -> Result<TablePolicies, AppError> {
    let regclass = db::regclass_literal(&schema, &table);
    let rls = introspect_rows(&state, &session_id, &db::rls_sql(&regclass)).await?;
    let rows = introspect_rows(&state, &session_id, &db::policies_sql(&regclass)).await?;
    Ok(TablePolicies {
        rls_enabled: rls.first().map(|r| cell_bool(r, 0)).unwrap_or(false),
        rls_forced: rls.first().map(|r| cell_bool(r, 1)).unwrap_or(false),
        policies: rows
            .into_iter()
            .map(|row| PolicyInfo {
                name: cell(&row, 0),
                command: cell(&row, 1),
                permissive: cell_bool(&row, 2),
                roles: row[3].clone(),
                using_expr: row[4].clone(),
                check_expr: row[5].clone(),
            })
            .collect(),
    })
}
