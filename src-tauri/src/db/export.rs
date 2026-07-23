//! Full-result export: runs SQL and writes the chosen statement's rows to a
//! file. CSV/JSON/Markdown stream row-by-row (constant memory, any size);
//! XLSX accumulates in the workbook and is capped by the sheet row limit.
//! Serialization mirrors the frontend copy/export formats (lib/export.ts).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

use futures_util::TryStreamExt;
use rust_xlsxwriter::{Format, Workbook};
use serde::Serialize;
use tokio_postgres::{Client, SimpleQueryMessage};

use crate::error::AppError;
use crate::format::csv_field;

/// Data rows fitting a sheet after the header (xlsx hard limit is 1_048_576).
const XLSX_MAX_DATA_ROWS: u64 = 1_048_575;

#[derive(Clone, Copy)]
pub enum ExportFormat {
    Csv,
    Json,
    Md,
    Xlsx,
}

impl ExportFormat {
    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s {
            "csv" => Ok(Self::Csv),
            "json" => Ok(Self::Json),
            "md" => Ok(Self::Md),
            "xlsx" => Ok(Self::Xlsx),
            other => Err(AppError::Msg(format!("unknown export format: {other}"))),
        }
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub rows: u64,
    /// The XLSX sheet row limit cut the result; other formats never truncate.
    pub truncated: bool,
    pub duration_ms: u64,
}

/// [`export_statement`] failure split by origin, so the caller keeps the tx
/// heuristic honest: only the database failing means the SQL itself failed —
/// a full disk or an unwritable path never aborted the server's transaction.
#[derive(Debug)]
pub enum ExportError {
    /// The server rejected/aborted the script (or the wire died mid-stream).
    Sql(AppError),
    /// Local failure — file IO, XLSX limits, "no result set". The SQL ran.
    Local(AppError),
}

impl From<ExportError> for AppError {
    fn from(e: ExportError) -> Self {
        match e {
            ExportError::Sql(e) | ExportError::Local(e) => e,
        }
    }
}

/// Runs `sql` and exports the rows of statement `statement_index` (same
/// numbering as `ExecResult.results`) into `path`. The whole result is
/// written — no row limit except the XLSX sheet cap.
pub async fn export_statement(
    client: &Client,
    sql: &str,
    statement_index: usize,
    format: ExportFormat,
    path: &str,
) -> Result<ExportResult, ExportError> {
    let sql_err = |e: tokio_postgres::Error| ExportError::Sql(e.into());
    let start = Instant::now();
    let stream = client.simple_query_raw(sql).await.map_err(sql_err)?;
    futures_util::pin_mut!(stream);

    let mut writer = Writer::create(format, path).map_err(ExportError::Local)?;
    let mut idx = 0usize; // statement index, advanced on CommandComplete
    let mut columns_seen = false;
    let mut rows = 0u64;
    let mut truncated = false;

    while let Some(msg) = stream.try_next().await.map_err(sql_err)? {
        match msg {
            SimpleQueryMessage::RowDescription(cols) if idx == statement_index => {
                writer
                    .begin(&cols.iter().map(|c| c.name().to_string()).collect::<Vec<_>>())
                    .map_err(ExportError::Local)?;
                columns_seen = true;
            }
            SimpleQueryMessage::Row(row) if idx == statement_index => {
                if !columns_seen {
                    // same tolerance as execute(): a Row without a preceding
                    // RowDescription still carries its column names
                    writer
                        .begin(
                            &row.columns().iter().map(|c| c.name().to_string()).collect::<Vec<_>>(),
                        )
                        .map_err(ExportError::Local)?;
                    columns_seen = true;
                }
                if writer.at_capacity(rows) {
                    // dropping the stream discards the rest of the response
                    truncated = true;
                    break;
                }
                writer
                    .row(&(0..row.len()).map(|i| row.get(i)).collect::<Vec<_>>())
                    .map_err(ExportError::Local)?;
                rows += 1;
            }
            SimpleQueryMessage::CommandComplete(_) => {
                if idx == statement_index {
                    break;
                }
                idx += 1;
            }
            _ => {}
        }
    }

    if !columns_seen {
        return Err(ExportError::Local(AppError::Msg(
            "the statement produced no result set to export".into(),
        )));
    }
    writer.finish(path).map_err(ExportError::Local)?;

    Ok(ExportResult {
        rows,
        truncated,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Writes an already-materialized result — the grid's current selection, with
/// staged edits applied — straight to `path` as XLSX. The streaming
/// [`export_statement`] re-runs the SQL server-side and so can't see a
/// client-side cell/row/column selection or unsaved edits; this takes the rows
/// verbatim from the frontend. Reuses [`Writer`] so the on-disk shape (bold
/// header, number heuristic, NULL→blank) matches the server export exactly.
pub fn write_rows_xlsx(
    path: &str,
    columns: &[String],
    rows: &[Vec<Option<String>>],
) -> Result<u64, AppError> {
    let mut writer = Writer::create(ExportFormat::Xlsx, path)?;
    writer.begin(columns)?;
    let mut n = 0u64;
    for row in rows {
        if writer.at_capacity(n) {
            break; // XLSX sheet cap — same silent truncation as the streamer
        }
        let vals: Vec<Option<&str>> = row.iter().map(|v| v.as_deref()).collect();
        writer.row(&vals)?;
        n += 1;
    }
    writer.finish(path)?;
    Ok(n)
}

enum Writer {
    Csv(BufWriter<File>),
    Json {
        out: BufWriter<File>,
        columns: Vec<String>,
        any: bool,
    },
    Md(BufWriter<File>),
    Xlsx {
        book: Box<Workbook>,
        next_row: u32,
    },
}

impl Writer {
    fn create(format: ExportFormat, path: &str) -> Result<Self, AppError> {
        Ok(match format {
            ExportFormat::Csv => Writer::Csv(BufWriter::new(File::create(path)?)),
            ExportFormat::Json => Writer::Json {
                out: BufWriter::new(File::create(path)?),
                columns: Vec::new(),
                any: false,
            },
            ExportFormat::Md => Writer::Md(BufWriter::new(File::create(path)?)),
            ExportFormat::Xlsx => {
                let mut book = Box::new(Workbook::new());
                book.add_worksheet();
                Writer::Xlsx { book, next_row: 0 }
            }
        })
    }

    /// Writes the header; must be called once before any [`Writer::row`].
    fn begin(&mut self, cols: &[String]) -> Result<(), AppError> {
        match self {
            Writer::Csv(out) => {
                let line =
                    cols.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(",");
                out.write_all(line.as_bytes())?;
            }
            Writer::Json { columns, .. } => {
                *columns = cols.to_vec();
            }
            Writer::Md(out) => {
                let head = cols.iter().map(|c| md_cell(c)).collect::<Vec<_>>();
                write!(out, "{}", md_line(&head))?;
                write!(out, "\n{}", md_line(&vec!["---".into(); cols.len()]))?;
            }
            Writer::Xlsx { book, next_row } => {
                u16::try_from(cols.len())
                    .map_err(|_| AppError::Msg("too many columns for XLSX".into()))?;
                let bold = Format::new().set_bold();
                let ws = book.worksheet_from_index(0).map_err(xlsx_err)?;
                for (c, name) in cols.iter().enumerate() {
                    ws.write_string_with_format(0, c as u16, name, &bold)
                        .map_err(xlsx_err)?;
                }
                *next_row = 1;
            }
        }
        Ok(())
    }

    /// True when the format can't take row `next` (0-based) — XLSX sheet cap.
    fn at_capacity(&self, next: u64) -> bool {
        matches!(self, Writer::Xlsx { .. }) && next >= XLSX_MAX_DATA_ROWS
    }

    fn row(&mut self, values: &[Option<&str>]) -> Result<(), AppError> {
        match self {
            Writer::Csv(out) => {
                let line = values
                    .iter()
                    .map(|v| csv_field(v.unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join(",");
                write!(out, "\r\n{line}")?;
            }
            Writer::Json { out, columns, any } => {
                out.write_all(if *any { b",\n  {" } else { b"[\n  {" })?;
                *any = true;
                for (i, (col, v)) in columns.iter().zip(values).enumerate() {
                    let sep = if i > 0 { "," } else { "" };
                    let val = match v {
                        Some(s) => jstr(s),
                        None => "null".into(),
                    };
                    write!(out, "{sep}\n    {}: {val}", jstr(col))?;
                }
                out.write_all(if columns.is_empty() { b"}" } else { b"\n  }" })?;
            }
            Writer::Md(out) => {
                let cells = values
                    .iter()
                    .map(|v| md_cell(v.unwrap_or_default()))
                    .collect::<Vec<_>>();
                write!(out, "\n{}", md_line(&cells))?;
            }
            Writer::Xlsx { book, next_row } => {
                let r = *next_row;
                *next_row += 1;
                let ws = book.worksheet_from_index(0).map_err(xlsx_err)?;
                for (c, v) in values.iter().enumerate() {
                    let Some(s) = v else { continue }; // NULL → blank cell
                    match xlsx_number(s) {
                        Some(n) => ws.write_number(r, c as u16, n).map_err(xlsx_err)?,
                        None => ws.write_string(r, c as u16, *s).map_err(xlsx_err)?,
                    };
                }
            }
        }
        Ok(())
    }

    fn finish(self, path: &str) -> Result<(), AppError> {
        match self {
            Writer::Csv(mut out) | Writer::Md(mut out) => out.flush()?,
            Writer::Json { mut out, any, .. } => {
                out.write_all(if any { b"\n]" } else { b"[]" })?;
                out.flush()?;
            }
            Writer::Xlsx { mut book, .. } => book.save(path).map_err(xlsx_err)?,
        }
        Ok(())
    }
}

fn xlsx_err(e: rust_xlsxwriter::XlsxError) -> AppError {
    AppError::Msg(format!("XLSX: {e}"))
}

/// JSON string literal (same escaping as JSON.stringify).
fn jstr(s: &str) -> String {
    serde_json::to_string(s).expect("string to JSON is infallible")
}

fn md_cell(v: &str) -> String {
    v.replace('|', "\\|").replace("\r\n", "<br>").replace('\n', "<br>")
}

fn md_line(cells: &[String]) -> String {
    format!("| {} |", cells.join(" | "))
}

/// Number for an XLSX cell — only when the text is reconstructible from the
/// f64 without loss ("1.50" and huge ids stay text, so nothing is corrupted).
fn xlsx_number(v: &str) -> Option<f64> {
    let n: f64 = v.parse().ok()?;
    (n.is_finite() && n.to_string() == v).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_all(format: ExportFormat, rows: &[Vec<Option<&str>>]) -> String {
        let path = std::env::temp_dir().join(format!("sql-kai-export-{}", uuid::Uuid::new_v4()));
        let path = path.to_str().unwrap();
        let mut w = Writer::create(format, path).unwrap();
        w.begin(&["id".into(), "name".into()]).unwrap();
        for r in rows {
            w.row(r).unwrap();
        }
        w.finish(path).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        std::fs::remove_file(path).unwrap();
        text
    }

    #[test]
    fn csv_quotes_and_crlf() {
        let text = write_all(
            ExportFormat::Csv,
            &[vec![Some("1"), Some("a,\"b\"")], vec![Some("2"), None]],
        );
        assert_eq!(text, "id,name\r\n1,\"a,\"\"b\"\"\"\r\n2,");
    }

    #[test]
    fn json_matches_stringify_shape() {
        let text = write_all(ExportFormat::Json, &[vec![Some("1"), None]]);
        assert_eq!(text, "[\n  {\n    \"id\": \"1\",\n    \"name\": null\n  }\n]");
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed[0]["name"], serde_json::Value::Null);
    }

    #[test]
    fn json_empty_result_is_empty_array() {
        assert_eq!(write_all(ExportFormat::Json, &[]), "[]");
    }

    #[test]
    fn md_escapes_pipes_and_newlines() {
        let text = write_all(ExportFormat::Md, &[vec![Some("a|b"), Some("x\ny")]]);
        assert_eq!(
            text,
            "| id | name |\n| --- | --- |\n| a\\|b | x<br>y |"
        );
    }

    #[test]
    fn xlsx_writes_a_zip_container() {
        let path = std::env::temp_dir().join(format!("sql-kai-export-{}.xlsx", uuid::Uuid::new_v4()));
        let path = path.to_str().unwrap();
        let mut w = Writer::create(ExportFormat::Xlsx, path).unwrap();
        w.begin(&["id".into(), "name".into()]).unwrap();
        w.row(&[Some("1"), Some("Alice")]).unwrap();
        w.row(&[None, Some("x\ny")]).unwrap();
        w.finish(path).unwrap();
        let bytes = std::fs::read(path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(bytes.starts_with(b"PK"), "xlsx must be a zip container");
    }

    #[test]
    fn write_rows_xlsx_from_selection() {
        let path =
            std::env::temp_dir().join(format!("sql-kai-sel-{}.xlsx", uuid::Uuid::new_v4()));
        let path = path.to_str().unwrap();
        let n = write_rows_xlsx(
            path,
            &["id".into(), "name".into()],
            &[
                vec![Some("1".into()), Some("Alice".into())],
                vec![Some("2".into()), None], // NULL → blank cell
            ],
        )
        .unwrap();
        let bytes = std::fs::read(path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(n, 2);
        assert!(bytes.starts_with(b"PK"), "xlsx must be a zip container");
    }

    #[test]
    fn xlsx_number_round_trip_only() {
        assert_eq!(xlsx_number("123"), Some(123.0));
        assert_eq!(xlsx_number("-1.5"), Some(-1.5));
        assert_eq!(xlsx_number("1.50"), None); // formatting would be lost
        assert_eq!(xlsx_number("9007199254740993"), None); // f64 precision loss
        assert_eq!(xlsx_number("NaN"), None);
        assert_eq!(xlsx_number("007"), None);
        assert_eq!(xlsx_number("abc"), None);
    }
}
