//! Postgres layer: connecting (optionally over an ssh tunnel), running SQL,
//! SQL-text utilities and pg_catalog introspection helpers.

mod connect;
mod exec;
mod export;
mod sqltext;
mod catalog;

pub use tokio_postgres::types::Type;

pub use catalog::{
    columns_sql, indexes_sql, policies_sql, quote_ident, quote_literal, regclass_literal,
    relations_sql, rls_sql, schema_dump_sql, table_ddl, triggers_sql, SchemaOptions, ENUMS_SQL,
    SCHEMA_DUMP_PARTS, TABLES_COUNTS_SQL, TABLES_SQL,
};
pub use connect::{connect, ConnectOptions, Connected, Session};
pub use exec::{
    cell, cell_bool, execute, execute_read_only, query_rows, query_scalar, statement_column_types,
    ExecResult, QueryExecutor, StatementResult,
};
pub use export::{
    export_statement, write_rows_xlsx, ExportError, ExportFormat, ExportResult,
};
pub use sqltext::{
    advance_tx, escapes_read_only_tx, is_tx_control_only, split_statements, TxStatus,
};
