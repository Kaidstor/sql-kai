//! Рендеры результатов: pretty-таблица, JSON (строки/типизированный), CSV,
//! tuples-only.

use sql_tauri_lib::db::{ExecResult, StatementResult, Type};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Table,
    /// JSON с типизацией значений по колонкам — рендерится print_exec_json.
    Json,
    Csv,
    Tuples,
}

pub fn print_exec(exec: &ExecResult, fmt: Format) {
    let mut first = true;
    for r in &exec.results {
        // Пустые columns = не-SELECT (INSERT/UPDATE/SET/...): в таблице
        // показываем счётчик, в машинных форматах пропускаем.
        if r.columns.is_empty() {
            if fmt == Format::Table {
                if let Some(n) = r.rows_affected {
                    println!("-- {n} row(s) affected");
                }
            }
            continue;
        }
        if !first {
            println!();
        }
        first = false;
        match fmt {
            Format::Table => print_table(&r.columns, &r.rows, r.truncated),
            Format::Csv => print_csv(r),
            Format::Tuples => {
                for row in &r.rows {
                    let vals: Vec<&str> = row.iter().map(|c| c.as_deref().unwrap_or("")).collect();
                    println!("{}", vals.join("|"));
                }
            }
            Format::Json => unreachable!("Json рендерится print_exec_json"),
        }
    }
}

/// --json: {results:[{columns, rows, rowsAffected, truncated}], durationMs},
/// значения приведены к JSON-типам по типам колонок из Parse
/// (`db::statement_column_types`). `stmt_types` идёт в порядке стейтментов и
/// сверяется с результатом по именам колонок; при несовпадении (или None)
/// значения того результата остаются строками. Возвращает число result-set'ов
/// с колонками, для которых типизация не удалась, — для предупреждения в stderr.
pub fn print_exec_json(exec: &ExecResult, stmt_types: &[Option<Vec<(String, Type)>>]) -> usize {
    use serde_json::{json, Value};
    let mut untyped = 0usize;
    let mut types_iter = stmt_types.iter();
    let results: Vec<Value> = exec
        .results
        .iter()
        .map(|r| {
            let types = types_iter.next().and_then(|t| t.as_ref()).filter(|cols| {
                cols.len() == r.columns.len()
                    && cols.iter().zip(&r.columns).all(|((n, _), c)| n == c)
            });
            if types.is_none() && !r.columns.is_empty() {
                untyped += 1;
            }
            let rows: Vec<Value> = r
                .rows
                .iter()
                .map(|row| {
                    Value::Array(
                        row.iter()
                            .enumerate()
                            .map(|(i, cell)| match (cell, types) {
                                (None, _) => Value::Null,
                                (Some(text), Some(cols)) => typed_value(text, &cols[i].1),
                                (Some(text), None) => Value::String(text.clone()),
                            })
                            .collect(),
                    )
                })
                .collect();
            json!({
                "columns": r.columns,
                "rows": rows,
                "rowsAffected": r.rows_affected,
                "truncated": r.truncated,
            })
        })
        .collect();
    let out = json!({ "results": results, "durationMs": exec.duration_ms });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
    untyped
}

/// Текст значения (simple-query отдаёт всё текстом) -> JSON-значение по типу
/// колонки. Всё, что не парсится или не мапится (в т.ч. [redacted] в числовой
/// колонке, NaN, numeric за пределами f64), остаётся строкой.
fn typed_value(text: &str, ty: &Type) -> serde_json::Value {
    use serde_json::Value;
    if *ty == Type::BOOL {
        return match text {
            "t" | "true" => Value::Bool(true),
            "f" | "false" => Value::Bool(false),
            _ => Value::String(text.to_string()),
        };
    }
    if [Type::INT2, Type::INT4, Type::INT8, Type::OID].contains(ty) {
        return text
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(text.to_string()));
    }
    if [Type::FLOAT4, Type::FLOAT8, Type::NUMERIC].contains(ty) {
        if let Ok(n) = text.parse::<i64>() {
            return Value::from(n);
        }
        return text
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(text.to_string()));
    }
    if *ty == Type::JSON || *ty == Type::JSONB {
        return serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()));
    }
    Value::String(text.to_string())
}

/// Таблица для интроспекции/списков: столбцы заданы кодом, не сервером.
pub fn print_rows(columns: &[&str], rows: &[Vec<Option<String>>], json: bool) {
    if json {
        let arr: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                let mut obj = serde_json::Map::new();
                for (i, c) in columns.iter().enumerate() {
                    let v = row
                        .get(i)
                        .and_then(|v| v.clone())
                        .map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null);
                    obj.insert((*c).to_string(), v);
                }
                serde_json::Value::Object(obj)
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr).unwrap());
        return;
    }
    let cols: Vec<String> = columns.iter().map(|s| s.to_string()).collect();
    print_table(&cols, rows, false);
}

fn print_table(columns: &[String], rows: &[Vec<Option<String>>], truncated: bool) {
    // Управляющие символы ломают выравнивание — экранируем переводы строк.
    let sanitize =
        |s: &str| s.replace('\r', "").replace('\n', "\\n").replace('\t', " ");
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            (0..columns.len())
                .map(|i| sanitize(row.get(i).and_then(|c| c.as_deref()).unwrap_or("")))
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = columns.iter().map(|c| c.chars().count()).collect();
    for row in &cells {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let render = |vals: &[String]| {
        vals.iter()
            .enumerate()
            .map(|(i, v)| format!("{:<w$}", v, w = widths[i]))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    println!("{}", render(columns).trim_end());
    println!(
        "{}",
        widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("-+-")
    );
    for row in &cells {
        println!("{}", render(row).trim_end());
    }
    let n = rows.len();
    println!(
        "({n} row{}{})",
        if n == 1 { "" } else { "s" },
        if truncated { ", truncated — увеличь --max-rows" } else { "" }
    );
}

fn print_csv(r: &StatementResult) {
    println!(
        "{}",
        r.columns.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(",")
    );
    for row in &r.rows {
        let vals: Vec<String> = row
            .iter()
            .map(|c| csv_field(c.as_deref().unwrap_or("")))
            .collect();
        println!("{}", vals.join(","));
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::typed_value;
    use serde_json::{json, Value};
    use sql_tauri_lib::db::Type;

    #[test]
    fn typed_value_converts_by_column_type() {
        assert_eq!(typed_value("3", &Type::INT8), json!(3));
        assert_eq!(typed_value("t", &Type::BOOL), json!(true));
        assert_eq!(typed_value("f", &Type::BOOL), json!(false));
        assert_eq!(typed_value("1.5", &Type::NUMERIC), json!(1.5));
        // целый numeric остаётся целым, не 42.0
        assert_eq!(typed_value("42", &Type::NUMERIC), json!(42));
        assert_eq!(
            typed_value("{\"a\": 1}", &Type::JSONB),
            json!({"a": 1})
        );
        // текстовая колонка с цифрами — строка, никакого угадывания
        assert_eq!(typed_value("3", &Type::TEXT), json!("3"));
    }

    #[test]
    fn typed_value_falls_back_to_string() {
        assert_eq!(
            typed_value("[redacted]", &Type::INT4),
            Value::String("[redacted]".into())
        );
        assert_eq!(typed_value("NaN", &Type::NUMERIC), Value::String("NaN".into()));
    }
}
