//! `sql-kai feedback [сообщение]` — собрать несекретную диагностику и составить
//! ссылку на предзаполненный issue в GitHub.
//!
//! Команда принципиально ничего не отправляет сама: она печатает URL, а issue
//! создаёт пользователь в браузере (и только после явного подтверждения мы этот
//! браузер открываем). Так же принципиально в диагностику не попадают хосты,
//! имена баз, профили и тексты запросов — issue публичный, а сообщение об
//! ошибке не стоит того, чтобы утащить туда карту чужой инфраструктуры.

use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use clap::Args;
use sql_kai_lib::error::AppError;
use sql_kai_lib::{store, vault};

use crate::broker_client;
use crate::cmd::doctor;
use crate::output::{self, Format, FormatArgs};

/// Форма нового issue репозитория (title/body — её штатные query-параметры).
const ISSUE_NEW_URL: &str = "https://github.com/Kaidstor/sql-kai/issues/new";
/// Длинный URL режут и браузер, и сам GitHub (~8 КБ на строку запроса), а
/// percent-encoding кириллицы раздувает текст втрое — держим запас.
const MESSAGE_CAP: usize = 4000;
/// Заголовок issue — первая строка сообщения; в GitHub он всё равно короткий.
const TITLE_CAP: usize = 100;

#[derive(Args)]
pub struct FeedbackArgs {
    /// Текст сообщения (без него — читается со stdin)
    message: Option<String>,
    #[command(flatten)]
    fmt: FormatArgs,
    /// Только напечатать ссылку, ничего не открывать и не спрашивать
    #[arg(long)]
    print_only: bool,
}

/// Строка диагностики: ключ для машин, подпись для человека и issue.
struct DiagRow {
    key: &'static str,
    label: &'static str,
    value: String,
}

fn row(key: &'static str, label: &'static str, value: impl Into<String>) -> DiagRow {
    DiagRow {
        key,
        label,
        value: value.into(),
    }
}

/// Домашний каталог в путях сворачивается в `~`: issue публичный, а имя
/// пользователя в пути — лишняя деталь.
fn tilde(path: &Path) -> String {
    let full = path.display().to_string();
    match dirs::home_dir() {
        Some(home) => match full.strip_prefix(&home.display().to_string()) {
            Some(rest) => format!("~{rest}"),
            None => full,
        },
        None => full,
    }
}

/// Версия ОС — через `sw_vers` (плюс архитектура из самого бинаря); не вышло —
/// честное «неизвестно», выдумывать номер сборки не из чего.
fn os_version() -> String {
    let arch = std::env::consts::ARCH;
    if cfg!(target_os = "macos") {
        let mut cmd = Command::new("sw_vers");
        vault::scrub_master_password_env(&mut cmd);
        let out = cmd.arg("-productVersion").stdin(Stdio::null()).output();
        if let Ok(out) = out {
            if out.status.success() {
                let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !v.is_empty() {
                    return format!("macOS {v} ({arch})");
                }
            }
        }
        return format!("macOS (версия неизвестна, {arch})");
    }
    format!("{} ({arch})", std::env::consts::OS)
}

/// Состояние сервера сессий: версия и разблокирован ли у него vault. Ходим
/// только в уже живые сокеты (connect/connect_holder), holder не поднимаем —
/// диагностика не должна плодить процессы.
async fn broker_state(gui: bool) -> String {
    let client = if gui {
        broker_client::connect().await
    } else {
        broker_client::connect_holder().await
    };
    match client {
        Some(b) => format!(
            "запущен ({}, vault {})",
            b.hello.server_version,
            if b.hello.vault_unlocked { "разблокирован" } else { "заперт" }
        ),
        None => "не запущен".to_string(),
    }
}

/// Всё, что уходит в issue помимо текста пользователя. Ничего идентифицирующего
/// инфраструктуру: только версии, способ установки и да/нет по локальным
/// подсистемам; профили представлены одним числом.
async fn collect_diag() -> Vec<DiagRow> {
    let install = doctor::detect_install();
    let profiles = store::load_profiles().map(|p| p.len()).unwrap_or(0);
    let mut rows = vec![
        row("cliVersion", "sql-kai CLI", env!("CARGO_PKG_VERSION")),
        row("os", "ОС", os_version()),
        row("install", "установка", install.label()),
        row("bin", "бинарь", tilde(&install.invoked)),
        row("gui", "GUI", broker_state(true).await),
        row("holder", "holder", broker_state(false).await),
        row(
            "vault",
            "vault",
            format!(
                "{}, cli trust {}, touch id {}",
                if vault::exists() { "есть" } else { "нет" },
                if vault::cli_trust_enrolled() { "вкл" } else { "выкл" },
                if vault::biometric_enrolled() { "вкл" } else { "выкл" },
            ),
        ),
        row("profiles", "профилей", profiles.to_string()),
    ];
    if let Some(note) = install.note() {
        rows.push(row("note", "внимание", note));
    }
    rows
}

/// Percent-encoding для query-параметров (RFC 3986: unreserved как есть,
/// остальные байты UTF-8 — %XX). Ради двух параметров тащить крейт не за чем.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Заголовок issue — первая непустая строка сообщения.
fn issue_title(message: &str) -> String {
    let first = message
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("отзыв о sql-kai");
    if first.chars().count() > TITLE_CAP {
        format!("{}…", first.chars().take(TITLE_CAP - 1).collect::<String>())
    } else {
        first.to_string()
    }
}

/// Тело issue: текст пользователя + свёрнутый блок диагностики. Блок оформлен
/// как код, чтобы GitHub не пытался разметить его содержимое.
fn issue_body(message: &str, diag: &[DiagRow]) -> String {
    let width = diag.iter().map(|r| r.label.chars().count()).max().unwrap_or(0);
    let lines: Vec<String> = diag
        .iter()
        .map(|r| format!("{:<width$} : {}", r.label, r.value, width = width))
        .collect();
    format!(
        "{message}\n\n<details><summary>Диагностика (sql-kai feedback)</summary>\n\n```\n{}\n```\n\n</details>\n",
        lines.join("\n")
    )
}

fn issue_url(title: &str, body: &str) -> String {
    format!(
        "{ISSUE_NEW_URL}?title={}&body={}",
        percent_encode(title),
        percent_encode(body)
    )
}

/// Сообщение из аргумента или со stdin (пайп). В терминале без аргумента ждать
/// ввода не начинаем — это выглядит как зависшая команда.
fn read_message(arg: Option<String>) -> Result<String, AppError> {
    let raw = match arg {
        Some(m) => m,
        None => {
            if std::io::stdin().is_terminal() {
                return Err(AppError::Msg(
                    "нет текста: передай сообщение аргументом (sql-kai feedback \"…\") или на stdin"
                        .into(),
                ));
            }
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };
    let msg = raw.trim();
    if msg.is_empty() {
        return Err(AppError::Msg("пустое сообщение — нечего отправлять".into()));
    }
    if msg.chars().count() > MESSAGE_CAP {
        let cut: String = msg.chars().take(MESSAGE_CAP).collect();
        return Ok(format!(
            "{cut}\n\n…(сообщение обрезано до {MESSAGE_CAP} символов — длинные логи лучше приложить в issue файлом)"
        ));
    }
    Ok(msg.to_string())
}

fn open_in_browser(url: &str) -> Result<(), AppError> {
    let bin = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let mut cmd = Command::new(bin);
    vault::scrub_master_password_env(&mut cmd);
    let status = cmd
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| AppError::Msg(format!("{bin}: {e}")))?;
    if !status.success() {
        return Err(AppError::Msg(format!(
            "{bin} вернул {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

pub async fn run(a: FeedbackArgs) -> Result<ExitCode, AppError> {
    let message = read_message(a.message.clone())?;
    let diag = collect_diag().await;
    let title = issue_title(&message);
    let url = issue_url(&title, &issue_body(&message, &diag));

    let fmt = a.fmt.pick();
    if fmt == Format::Json {
        let mut obj = serde_json::Map::new();
        for r in &diag {
            obj.insert(r.key.to_string(), serde_json::Value::String(r.value.clone()));
        }
        let out = serde_json::json!({
            "url": url,
            "title": title,
            "diagnostics": serde_json::Value::Object(obj),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return Ok(ExitCode::SUCCESS);
    }
    if fmt == Format::Csv || fmt == Format::Tuples {
        let mut rows: Vec<Vec<Option<String>>> = diag
            .iter()
            .map(|r| vec![Some(r.key.to_string()), Some(r.value.clone())])
            .collect();
        rows.push(vec![Some("url".into()), Some(url)]);
        output::print_rows(&["key", "value"], &rows, fmt);
        return Ok(ExitCode::SUCCESS);
    }

    // Человеку сначала показываем, что именно уедет в публичный issue, и лишь
    // потом ссылку: согласие имеет смысл только осознанное.
    let width = diag.iter().map(|r| r.label.chars().count()).max().unwrap_or(0);
    eprintln!("в issue уйдёт (кроме твоего текста):");
    for r in &diag {
        eprintln!("  {:<width$} : {}", r.label, r.value, width = width);
    }
    eprintln!();
    println!("{url}");

    if a.print_only {
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!(
        "\nsql-kai ничего никуда не отправляет: issue создашь ты сам в браузере кнопкой Submit."
    );
    if !std::io::stdin().is_terminal() {
        eprintln!("нет TTY для подтверждения — открой ссылку сам (или добавь --print-only)");
        return Ok(ExitCode::SUCCESS);
    }
    eprint!("открыть ссылку в браузере? [y/N] ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if !matches!(line.trim(), "y" | "Y" | "yes") {
        eprintln!("не открываю — ссылка выше");
        // Отказ открыть браузер не делает команду неуспешной: ссылка составлена
        // и напечатана, а это и есть её работа.
        return Ok(ExitCode::SUCCESS);
    }
    open_in_browser(&url)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::{issue_title, issue_url, percent_encode, DiagRow};

    #[test]
    fn percent_encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(percent_encode("abcXYZ-._~09"), "abcXYZ-._~09");
        assert_eq!(percent_encode("a b&c=d#e"), "a%20b%26c%3Dd%23e");
        assert_eq!(percent_encode("+/?"), "%2B%2F%3F");
        // кириллица — по байтам UTF-8
        assert_eq!(percent_encode("да"), "%D0%B4%D0%B0");
    }

    #[test]
    fn issue_title_takes_the_first_meaningful_line() {
        assert_eq!(issue_title("\n\nупал экспорт\nподробности ниже"), "упал экспорт");
        let long = "я".repeat(200);
        let title = issue_title(&long);
        assert_eq!(title.chars().count(), super::TITLE_CAP);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn issue_url_carries_title_and_body() {
        let diag = vec![DiagRow {
            key: "cliVersion",
            label: "sql-kai CLI",
            value: "1.2.3".into(),
        }];
        let url = issue_url("падает q", &super::issue_body("падает q", &diag));
        assert!(url.starts_with("https://github.com/Kaidstor/sql-kai/issues/new?title="));
        assert!(url.contains("&body="));
        // ни одного «сырого» символа, ломающего query-строку
        assert!(!url[url.find('?').unwrap()..].contains(' '));
        assert!(url.contains(&percent_encode("1.2.3")));
    }
}
