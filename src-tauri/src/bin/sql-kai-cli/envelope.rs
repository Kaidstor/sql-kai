//! Отказ команды: текстом в stderr, а под `--json` — конвертом семьи kai-cli в
//! stdout: `{v, command, exit, data: null, warning?, error: {kind, message}}`.
//!
//! Успешный `--json` пока печатает данные без конверта (форма, которую читают
//! потребители `q --json`), поэтому исход под `--json` различается полем `v`:
//! есть — это отказ.

use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::OnceLock;

use clap::ArgMatches;
use serde_json::json;

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

pub fn render_failure(
    command: &str,
    exit: u8,
    kind: &str,
    message: &str,
    warning: &[String],
) -> String {
    let mut env = json!({
        "v": VERSION,
        "command": command,
        "exit": exit,
        "data": null,
    });
    if !warning.is_empty() {
        env["warning"] = json!(warning);
    }
    env["error"] = json!({ "kind": kind, "message": message });
    serde_json::to_string_pretty(&env).expect("конверт сериализуется")
}

/// Отказ с кодом 1. `kind` — машинный класс (`db`, `read_only`, `app`, …),
/// `hints` — подсказки: в тексте идут строками после ошибки, в конверте —
/// полем `warning`.
pub fn fail(kind: &str, message: &str, hints: &[String]) -> ExitCode {
    match json_command() {
        Some(command) => println!("{}", render_failure(command, 1, kind, message, hints)),
        None => {
            eprintln!("sql-kai: {message}");
            for h in hints {
                eprintln!("{h}");
            }
        }
    }
    ExitCode::FAILURE
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
