//! Рендеры результатов: pretty-таблица, JSON, CSV, tuples-only.

use sql_tauri_lib::db::{ExecResult, StatementResult};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Table,
    Json,
    Csv,
    Tuples,
}

pub fn print_exec(exec: &ExecResult, fmt: Format) {
    if fmt == Format::Json {
        println!("{}", serde_json::to_string_pretty(exec).unwrap());
        return;
    }
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
            Format::Json => unreachable!(),
        }
    }
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
