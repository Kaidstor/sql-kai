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
/// Предел на саму ссылку: длинный URL режут и браузер, и сам GitHub (~8 КБ на
/// строку запроса) — держим запас. Это и есть настоящее ограничение, см.
/// [`fit_issue_url`].
const URL_CAP: usize = 6000;
/// Грубый предел на текст — только чтобы не гонять цикл подгонки по мегабайтам
/// со stdin; в лимит ссылки его укладывает [`fit_issue_url`].
const MESSAGE_CAP: usize = 20_000;
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
    tilde_from(path, dirs::home_dir().as_deref())
}

/// Сворачивание по компонентам пути, а не по подстроке: у дома `/Users/kai`
/// путь `/Users/kaiden/bin/sql-kai` иначе превращался в `~den/bin/sql-kai` —
/// в отчёте оказывался путь, которого на машине нет.
fn tilde_from(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
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

/// Ссылка, которая точно уложится в лимит. Резать надо собранный URL, а не
/// сообщение: одна кириллическая буква — это 2 байта UTF-8, то есть 6 символов
/// после percent-encoding, поэтому «4000 символов» превращались в ~24 КБ
/// query-строки, и длинный отзыв по-русски получал от GitHub 414.
fn fit_issue_url(title: &str, message: &str, diag: &[DiagRow]) -> String {
    let total = message.chars().count();
    let mut keep = total;
    loop {
        let text: String = message.chars().take(keep).collect();
        let text = if keep < total {
            format!("{text}\n\n…(обрезано: ссылка не влезала в лимит — длинные логи лучше приложить в issue файлом)")
        } else {
            text
        };
        let url = issue_url(title, &issue_body(&text, diag));
        // keep == 0 — не влезает уже одна диагностика; она мала и ограничена, но
        // ссылку всё равно отдаём: лучше длинная, чем никакая.
        if url.len() <= URL_CAP || keep == 0 {
            return url;
        }
        let over = url.len() - URL_CAP;
        keep = keep.saturating_sub(over.div_ceil(6).max(16));
    }
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
        return Ok(cut);
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
    let url = fit_issue_url(&title, &message, &diag);

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
    if !crate::input::is_interactive() {
        eprintln!("нет TTY для подтверждения — открой ссылку сам (или добавь --print-only)");
        return Ok(ExitCode::SUCCESS);
    }
    if !crate::input::confirm("открыть ссылку в браузере?", "--print-only")? {
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
    use super::{fit_issue_url, issue_title, issue_url, percent_encode, tilde_from, DiagRow};
    use std::path::Path;

    fn diag_row() -> Vec<DiagRow> {
        vec![DiagRow {
            key: "cliVersion",
            label: "sql-kai CLI",
            value: "1.2.3".into(),
        }]
    }

    /// Ограничение — на длину ссылки, а не на число символов: кириллица после
    /// percent-encoding занимает 6 символов на букву, и раньше длинный отзыв
    /// по-русски давал ~24 КБ query-строки при лимите GitHub ~8 КБ.
    #[test]
    fn issue_url_fits_the_length_limit() {
        let diag = diag_row();
        let long = "ошибка ".repeat(2000);
        let url = fit_issue_url("падает q", &long, &diag);
        assert!(url.len() <= super::URL_CAP, "{} байт", url.len());
        assert!(url.contains(&percent_encode("обрезано")));
        // короткое сообщение не режется вовсе
        let short = fit_issue_url("падает q", "падает q на проде", &diag);
        assert!(short.contains(&percent_encode("падает q на проде")));
        assert!(!short.contains(&percent_encode("обрезано")));
    }

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

    /// Дом сворачивается по границе компонента: сосед по /Users с домом в
    /// префиксе имени не должен превращаться в `~den/…`.
    #[test]
    fn tilde_folds_home_by_path_components() {
        let home = Path::new("/Users/kai");
        assert_eq!(
            tilde_from(Path::new("/Users/kai/.local/bin/sql-kai"), Some(home)),
            "~/.local/bin/sql-kai"
        );
        assert_eq!(tilde_from(Path::new("/Users/kai"), Some(home)), "~");
        // чужой дом с тем же префиксом остаётся как есть
        assert_eq!(
            tilde_from(Path::new("/Users/kaiden/bin/sql-kai"), Some(home)),
            "/Users/kaiden/bin/sql-kai"
        );
        assert_eq!(
            tilde_from(Path::new("/Applications/sql-kai.app"), Some(home)),
            "/Applications/sql-kai.app"
        );
        // дома нет — путь без изменений
        assert_eq!(
            tilde_from(Path::new("/Users/kai/bin/sql-kai"), None),
            "/Users/kai/bin/sql-kai"
        );
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
