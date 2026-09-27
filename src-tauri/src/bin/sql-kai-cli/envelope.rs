//! Вывод под `--json` — конверт семьи kai-cli в stdout:
//! `{v, command, exit, data, warning?, error}`. Успех — данные команды в `data`
//! и `error: null`; отказ — `error: {kind, message}`, `data` обычно `null`.
//! Без `--json` отказ идёт текстом в stderr. Код выхода отказа выводится из
//! `kind` ([`exit_for`]) и в текстовом режиме тот же.

use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::OnceLock;

use clap::ArgMatches;
use serde_json::{json, Value};

pub const VERSION: u32 = 1;

static JSON_COMMAND: OnceLock<Option<String>> = OnceLock::new();

/// Путь подкоманды (`q`, `saved list`), если у неё поднят `--json`.
pub fn json_command_of(m: &ArgMatches) -> Option<String> {
    let mut path = Vec::new();
    let mut cur = m;
    while let Some((name, sub)) = cur.subcommand() {
        path.push(name);
        cur = sub;
    }
    matches!(cur.try_get_one::<bool>("json"), Ok(Some(true))).then(|| path.join(" "))
}

pub fn set_json_command(command: Option<String>) {
    let _ = JSON_COMMAND.set(command);
}

fn json_command() -> Option<&'static str> {
    JSON_COMMAND.get().and_then(|c| c.as_deref())
}

/// Код выхода по классу отказа — таблица семьи kai-cli: 2 — инструмент,
/// аргументы, настройки, доступ; 3 — не найдено; 4 — сервер не ответил в срок,
/// повтор безопасен. Остальное (ошибка SQL, откат, потеря сессии) — 1.
pub fn exit_for(kind: &str) -> u8 {
    match kind {
        "usage" | "config" | "auth" | "network" | "prod_guard" => 2,
        "not_found" => 3,
        "timeout" => 4,
        _ => 1,
    }
}

fn render(
    command: &str,
    exit: u8,
    data: Value,
    warning: &[String],
    error: Option<(&str, &str)>,
) -> String {
    let mut env = json!({
        "v": VERSION,
        "command": command,
        "exit": exit,
        "data": data,
    });
    if !warning.is_empty() {
        env["warning"] = json!(warning);
    }
    env["error"] = match error {
        Some((kind, message)) => json!({ "kind": kind, "message": message }),
        None => Value::Null,
    };
    serde_json::to_string_pretty(&env).expect("конверт сериализуется")
}

fn render_success(command: &str, data: Value, warning: &[String]) -> String {
    render(command, 0, data, warning, None)
}

/// Успех под `--json`: данные команды — в `data`, код 0. Зовётся только в
/// JSON-ветке команды; `warning` — то, что в текстовом режиме ушло бы в stderr.
pub fn print_success(data: Value, warning: &[String]) {
    println!(
        "{}",
        render_success(json_command().unwrap_or_default(), data, warning)
    );
}

/// [`print_success`] для сериализуемых структур команды, без предупреждений.
pub fn print_data<T: serde::Serialize + ?Sized>(data: &T) {
    print_success(
        serde_json::to_value(data).expect("данные команды сериализуются"),
        &[],
    );
}

pub fn render_failure(
    command: &str,
    exit: u8,
    kind: &str,
    message: &str,
    warning: &[String],
) -> String {
    render_failure_with(command, exit, kind, message, warning, Value::Null)
}

fn render_failure_with(
    command: &str,
    exit: u8,
    kind: &str,
    message: &str,
    warning: &[String],
    data: Value,
) -> String {
    render(command, exit, data, warning, Some((kind, message)))
}

/// Отказ с кодом по [`exit_for`]. `kind` — машинный класс (`db`, `read_only`,
/// `not_found`, …), `hints` — подсказки: в тексте идут строками после ошибки,
/// в конверте — полем `warning`.
pub fn fail(kind: &str, message: &str, hints: &[String]) -> ExitCode {
    fail_with_data(kind, message, hints, Value::Null)
}

/// Отказ, у которого есть что показать: `doctor` отдаёт в `data` таблицу
/// проверок, чтобы было видно, какой профиль и какой источник не прошли.
/// В текстовом режиме `data` уже напечатана командой.
pub fn fail_with_data(kind: &str, message: &str, hints: &[String], data: Value) -> ExitCode {
    let exit = exit_for(kind);
    match json_command() {
        Some(command) => println!(
            "{}",
            render_failure_with(command, exit, kind, message, hints, data)
        ),
        None => {
            eprintln!("sql-kai: {message}");
            for h in hints {
                eprintln!("{h}");
            }
        }
    }
    ExitCode::from(exit)
}

/// Ошибка разбора clap случилась раньше, чем появились `ArgMatches`, поэтому
/// `--json` ищется в самом argv (до `--`).
pub fn argv_wants_json(args: &[OsString]) -> bool {
    args.iter()
        .skip(1)
        .take_while(|a| *a != "--")
        .any(|a| a == "--json")
}

/// Текст ошибки clap без префикса `error:` и без хвоста Usage/`--help`.
pub fn usage_message(err: &clap::Error) -> String {
    let text = err.render().to_string();
    let text = text.strip_prefix("error: ").unwrap_or(&text);
    let head = text.split("\nUsage:").next().unwrap_or(text);
    head.split("\nFor more information")
        .next()
        .unwrap_or(head)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn failure_envelope_has_the_family_shape() {
        let out = render_failure("q", 1, "db", "ERROR: relation \"x\" does not exist", &[]);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["command"], "q");
        assert_eq!(v["exit"], 1);
        assert!(v["data"].is_null());
        assert!(v.get("warning").is_none());
        assert_eq!(v["error"]["kind"], "db");
        assert_eq!(
            v["error"]["message"],
            "ERROR: relation \"x\" does not exist"
        );
        // отступ 2, порядок полей как у эталона
        assert!(out.starts_with("{\n  \"v\": 1,\n  \"command\""), "{out}");
    }

    #[test]
    fn success_envelope_wraps_data_unchanged() {
        let data = json!({"results": [{"columns": ["n"], "rows": [[1]]}], "durationMs": 3});
        let out = render_success("q", data.clone(), &[]);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["command"], "q");
        assert_eq!(v["exit"], 0);
        assert_eq!(v["data"], data);
        assert!(v.get("warning").is_none());
        assert!(v["error"].is_null());
        assert!(out.starts_with("{\n  \"v\": 1,\n  \"command\""), "{out}");
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["v", "command", "exit", "data", "error"]);
    }

    #[test]
    fn success_envelope_carries_warnings_and_arrays() {
        let out = render_success("saved list", json!([]), &["w".into()]);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["data"], json!([]));
        assert_eq!(v["warning"], json!(["w"]));
        assert!(v["error"].is_null());
    }

    #[test]
    fn exit_codes_follow_the_family_table() {
        for (kind, exit) in [
            ("usage", 2),
            ("config", 2),
            ("auth", 2),
            ("network", 2),
            ("prod_guard", 2),
            ("not_found", 3),
            ("timeout", 4),
            ("db", 1),
            ("read_only", 1),
            ("refused", 1),
            ("connection_lost", 1),
            ("app", 1),
        ] {
            assert_eq!(exit_for(kind), exit, "{kind}");
        }
    }

    #[test]
    fn failure_can_carry_data() {
        let out = render_failure_with("doctor", 2, "auth", "m", &[], json!([{"name": "x"}]));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["exit"], 2);
        assert_eq!(v["data"][0]["name"], "x");
    }

    #[test]
    fn hints_go_to_warning() {
        let out = render_failure("q", 1, "read_only", "m", &["hint: --write".into()]);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["warning"], json!(["hint: --write"]));
    }

    #[test]
    fn json_flag_is_found_before_double_dash_only() {
        let a = |l: &[&str]| l.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(argv_wants_json(&a(&[
            "sql-kai", "q", "x", "--json", "--bogus"
        ])));
        assert!(!argv_wants_json(&a(&["sql-kai", "q", "x", "--", "--json"])));
        assert!(!argv_wants_json(&a(&["sql-kai", "q", "x", "--csv"])));
    }
}
