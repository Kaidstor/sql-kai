//! `sql-kai mcp install [клиент]` / `sql-kai mcp status` — прописать sql-kai
//! MCP-сервером в конфиг AI-клиента (Claude Code, Codex, Cursor, VS Code,
//! Windsurf, Gemini CLI) и показать, где он уже прописан.
//!
//! Команда правит файлы ВНЕ репозитория, поэтому она подчёркнуто осторожна:
//! без явного имени клиента ничего не пишет (показывает список), всегда
//! печатает путь изменяемого файла, сливается с существующей конфигурацией
//! (чужие серверы не трогает), делает `.bak` перед записью, пишет атомарно
//! (tmp + rename, как store) и перечитывает результат для проверки.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use serde_json::{json, Map, Value};
use sql_kai_lib::error::AppError;
use sql_kai_lib::fsio;

use crate::output::{self, FormatArgs};
use crate::session;

/// Имя записи в конфиге клиента по умолчанию.
const DEFAULT_NAME: &str = "sql-kai";

#[derive(Args)]
pub struct InstallArgs {
    /// Клиент: claude-code, claude-desktop, codex, cursor, vscode, windsurf,
    /// gemini. Без аргумента — только список клиентов, ничего не пишется
    client: Option<String>,
    /// Закрепить сервер за одним профилем (по умолчанию — все профили)
    #[arg(long, value_name = "ALIAS")]
    profile: Option<String>,
    /// Имя записи в конфиге клиента
    #[arg(long, default_value = DEFAULT_NAME)]
    name: String,
    /// Путь к бинарю sql-kai в конфиге (по умолчанию — текущий)
    #[arg(long, value_name = "PATH")]
    command: Option<PathBuf>,
    /// Не делать .bak-копию конфига перед записью
    #[arg(long)]
    no_backup: bool,
    /// Показать, что будет записано, и выйти
    #[arg(long)]
    dry_run: bool,
    /// Перезаписать существующую запись с тем же именем
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
pub struct StatusArgs {
    /// Имя записи, которую искать в конфигах
    #[arg(long, default_value = DEFAULT_NAME)]
    name: String,
    #[command(flatten)]
    fmt: FormatArgs,
}

/// Формат, в котором клиент держит список MCP-серверов.
#[derive(Clone, Copy, PartialEq)]
enum Layout {
    /// `{ "mcpServers": { "<имя>": { "command": …, "args": […] } } }`
    McpServers,
    /// VS Code: `{ "servers": { "<имя>": { "type": "stdio", … } } }`
    VsCodeServers,
    /// Codex: TOML `[mcp_servers.<имя>]`. Правим текстом — toml-парсера в
    /// зависимостях нет, а тащить его ради одной секции незачем.
    CodexToml,
}

struct ClientSpec {
    key: &'static str,
    title: &'static str,
    layout: Layout,
    /// Путь конфига относительно домашнего каталога.
    rel_path: &'static str,
}

/// Клиенты, чей формат конфига известен наверняка. Пути — пользовательские
/// (не проектные): install ставит сервер один раз на все проекты.
fn clients() -> Vec<ClientSpec> {
    let vscode = if cfg!(target_os = "macos") {
        "Library/Application Support/Code/User/mcp.json"
    } else {
        ".config/Code/User/mcp.json"
    };
    let claude_desktop = if cfg!(target_os = "macos") {
        "Library/Application Support/Claude/claude_desktop_config.json"
    } else {
        ".config/Claude/claude_desktop_config.json"
    };
    vec![
        ClientSpec {
            key: "claude-code",
            title: "Claude Code",
            layout: Layout::McpServers,
            rel_path: ".claude.json",
        },
        ClientSpec {
            key: "claude-desktop",
            title: "Claude Desktop",
            layout: Layout::McpServers,
            rel_path: claude_desktop,
        },
        ClientSpec {
            key: "codex",
            title: "Codex CLI",
            layout: Layout::CodexToml,
            rel_path: ".codex/config.toml",
        },
        ClientSpec {
            key: "cursor",
            title: "Cursor",
            layout: Layout::McpServers,
            rel_path: ".cursor/mcp.json",
        },
        ClientSpec {
            key: "vscode",
            title: "VS Code",
            layout: Layout::VsCodeServers,
            rel_path: vscode,
        },
        ClientSpec {
            key: "windsurf",
            title: "Windsurf",
            layout: Layout::McpServers,
            rel_path: ".codeium/windsurf/mcp_config.json",
        },
        ClientSpec {
            key: "gemini",
            title: "Gemini CLI",
            layout: Layout::McpServers,
            rel_path: ".gemini/settings.json",
        },
    ]
}

impl ClientSpec {
    fn path(&self) -> Result<PathBuf, AppError> {
        let home = dirs::home_dir()
            .ok_or_else(|| AppError::Msg("не удалось определить домашний каталог".into()))?;
        Ok(home.join(self.rel_path))
    }

    /// Ключ верхнего уровня со списком серверов (только JSON-клиенты).
    fn wrapper(&self) -> &'static str {
        match self.layout {
            Layout::VsCodeServers => "servers",
            _ => "mcpServers",
        }
    }
}

/// Как sql-kai будет запускаться из конфига клиента.
struct ServerEntry {
    command: String,
    args: Vec<String>,
}

impl ServerEntry {
    fn json(&self, layout: Layout) -> Value {
        let mut obj = Map::new();
        if layout == Layout::VsCodeServers {
            obj.insert("type".into(), json!("stdio"));
        }
        obj.insert("command".into(), json!(self.command));
        obj.insert("args".into(), json!(self.args));
        Value::Object(obj)
    }

    fn toml(&self, name: &str) -> String {
        let args = self
            .args
            .iter()
            .map(|a| toml_string(a))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "[mcp_servers.{}]\ncommand = {}\nargs = [{}]\n",
            toml_key(name),
            toml_string(&self.command),
            args
        )
    }

    fn display(&self) -> String {
        format!("{} {}", self.command, self.args.join(" "))
    }
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Голый ключ TOML допустим только для [A-Za-z0-9_-]; всё прочее — в кавычки.
fn toml_key(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        s.to_string()
    } else {
        toml_string(s)
    }
}

pub fn install(a: InstallArgs) -> Result<ExitCode, AppError> {
    let specs = clients();
    let Some(key) = a.client.as_deref() else {
        println!("укажи клиента: sql-kai mcp install <клиент>\n");
        print_status_table(&specs, &a.name, output::Format::Table)?;
        println!(
            "\nничего не изменено. Пример: sql-kai mcp install claude-code{}",
            a.profile
                .as_deref()
                .map(|p| format!(" --profile {p}"))
                .unwrap_or_default()
        );
        return Ok(ExitCode::SUCCESS);
    };
    let spec = specs
        .iter()
        .find(|c| c.key.eq_ignore_ascii_case(key))
        .ok_or_else(|| {
            AppError::Msg(format!(
                "неизвестный клиент '{key}'; доступны: {}",
                specs.iter().map(|c| c.key).collect::<Vec<_>>().join(", ")
            ))
        })?;

    let command = match &a.command {
        Some(p) => p.clone(),
        None => std::env::current_exe().map_err(AppError::Io)?,
    };
    let mut args = vec!["mcp".to_string()];
    if let Some(alias) = &a.profile {
        // валидируем сразу: молча записанный неверный алиас всплывёт только
        // при первом запросе агента
        let profile = session::resolve_profile(alias)?;
        println!("профиль: {} ({})", profile.name, profile.database);
        args.push(alias.clone());
    }
    let entry = ServerEntry {
        command: command.to_string_lossy().to_string(),
        args,
    };

    let path = spec.path()?;
    println!("клиент: {} ({})", spec.title, spec.key);
    println!("файл:   {}", path.display());
    println!("запись: {} = {}", a.name, entry.display());

    if a.dry_run {
        println!("--dry-run: файл не изменён");
        return Ok(ExitCode::SUCCESS);
    }

    // Мы читаем и переписываем файл, которым владеет чужой процесс. Запущенный
    // клиент (Claude Code держит ~/.claude.json в памяти и сохраняет его
    // целиком) откатит нашу запись своей следующей — предупредить об этом
    // дешевле, чем разбираться, почему сервер «не прописался».
    if path.exists() {
        println!(
            "если {} сейчас запущен — закрой его: клиент может перезаписать конфиг своей копией",
            spec.title
        );
    }

    let changed = match spec.layout {
        Layout::CodexToml => install_toml(&path, &a, &entry)?,
        _ => install_json(spec, &path, &a, &entry)?,
    };
    if changed {
        println!("готово");
    }
    Ok(ExitCode::SUCCESS)
}

/// Слияние с существующим JSON-конфигом: чужие серверы и любые прочие ключи
/// файла сохраняются как есть. Возвращает false, если писать не пришлось.
fn install_json(
    spec: &ClientSpec,
    path: &Path,
    a: &InstallArgs,
    entry: &ServerEntry,
) -> Result<bool, AppError> {
    let existed = path.exists();
    let stamp = file_stamp(path);
    let mut root: Value = if existed {
        let text = std::fs::read_to_string(path).map_err(AppError::Io)?;
        if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text).map_err(|e| {
                AppError::Msg(format!(
                    "конфиг {} не разбирается как JSON ({e}) — возможно, это JSON с комментариями; \
                     допиши запись вручную",
                    path.display()
                ))
            })?
        }
    } else {
        json!({})
    };
    if !root.is_object() {
        return Err(AppError::Msg(format!(
            "конфиг {} — не JSON-объект, не трогаю",
            path.display()
        )));
    }

    let wrapper = spec.wrapper();
    let servers = root
        .as_object_mut()
        .expect("проверено выше")
        .entry(wrapper)
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        return Err(AppError::Msg(format!(
            "ключ \"{wrapper}\" в {} — не объект, не трогаю",
            path.display()
        )));
    }
    let new_entry = entry.json(spec.layout);
    if let Some(old) = servers.get(&a.name) {
        if same_entry(old, &new_entry) {
            println!("уже настроено — файл не изменён");
            return Ok(false);
        }
        if !a.force {
            return Err(AppError::Msg(format!(
                "в {} уже есть сервер \"{}\" с другой командой ({}) — перезаписать: --force",
                path.display(),
                a.name,
                old.get("command").and_then(Value::as_str).unwrap_or("?")
            )));
        }
        println!("перезаписываю существующую запись (--force)");
    }
    servers
        .as_object_mut()
        .expect("проверено выше")
        .insert(a.name.clone(), new_entry);

    let mut text = serde_json::to_string_pretty(&root)
        .map_err(|e| AppError::Msg(format!("сериализация конфига: {e}")))?;
    text.push('\n');

    ensure_unchanged(path, stamp)?;
    let backup = backup_file(path, a.no_backup || !existed)?;
    write_config(path, text.as_bytes())?;

    // перечитываем записанное: битый конфиг клиента хуже отсутствующего
    let check = std::fs::read_to_string(path).map_err(AppError::Io)?;
    let parsed: Result<Value, _> = serde_json::from_str(&check);
    let ok = parsed
        .as_ref()
        .ok()
        .and_then(|v| v.get(wrapper))
        .and_then(|s| s.get(&a.name))
        .is_some();
    if !ok {
        restore(path, backup.as_deref(), existed)?;
        return Err(verify_failed(path, backup.as_deref(), existed));
    }
    Ok(true)
}

/// Заголовок секции нашего сервера — в любом написании, допустимом в TOML:
/// `[mcp_servers.sql-kai]`, `[mcp_servers."sql-kai"]`, с пробелами вокруг точки.
/// Сравнение строкой пропускало квотированный вариант: рядом с ним добавлялась
/// вторая секция того же имени, наша проверка «секция ровно одна» этого не
/// видела, а TOML-парсер отвергал файл целиком как дубль таблицы.
fn is_server_header(line: &str, name: &str) -> bool {
    let Some(inner) = line
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
    else {
        return false;
    };
    let Some(rest) = inner.trim_start().strip_prefix("mcp_servers") else {
        return false;
    };
    // ключ не режем по точке: `[mcp_servers."a.b"]` — одно имя, а не два уровня
    let Some(key) = rest.trim_start().strip_prefix('.') else {
        return false;
    };
    let key = key.trim();
    key.strip_prefix('"')
        .and_then(|k| k.strip_suffix('"'))
        .or_else(|| key.strip_prefix('\'').and_then(|k| k.strip_suffix('\'')))
        .unwrap_or(key)
        == name
}

/// Для каждой строки файла: начинается ли она ВНЕ многострочного литерала TOML
/// (`"""…"""`, `'''…'''`). Без этого построчный скан принимает содержимое
/// чужого значения за разметку: `foo = """\n[mcp_servers.sql-kai]\n"""` — не
/// заголовок секции, а текст, и --force вырезал бы кусок из середины литерала.
///
/// Полноценный TOML-парсер тут не нужен (и его нет в зависимостях): заголовок
/// секции — всегда отдельная строка, поэтому достаточно знать, где строка
/// начинается.
fn code_lines(text: &str) -> Vec<bool> {
    let mut flags = Vec::new();
    let mut open: Option<&'static str> = None;
    for line in text.lines() {
        flags.push(open.is_none());
        advance(line, &mut open);
    }
    flags
}

/// Прокручивает состояние скана по одной строке: `open` — делимитер
/// многострочного литерала, внутри которого мы оказались к её концу.
fn advance(line: &str, open: &mut Option<&'static str>) {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if let Some(delim) = *open {
            // в """…""" обратный слэш экранирует и кавычку; в '''…''' escape'ов нет
            if delim == "\"\"\"" && b[i] == b'\\' {
                i += 2;
            } else if b[i..].starts_with(delim.as_bytes()) {
                *open = None;
                i += delim.len();
            } else {
                i += 1;
            }
            continue;
        }
        match b[i] {
            b'#' => return, // комментарий до конца строки
            b'"' if b[i..].starts_with(b"\"\"\"") => {
                *open = Some("\"\"\"");
                i += 3;
            }
            b'\'' if b[i..].starts_with(b"'''") => {
                *open = Some("'''");
                i += 3;
            }
            // однострочный литерал обязан закрыться на этой же строке: если он
            // не закрыт, файл — не TOML, и тащить состояние дальше вреднее,
            // чем считать следующую строку разметкой
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'\'' => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
}

/// Номера строк с заголовком нашей секции — только там, где TOML читает
/// разметку, а не содержимое многострочного литерала.
fn server_header_lines(text: &str, name: &str) -> Vec<usize> {
    let code = code_lines(text);
    text.lines()
        .enumerate()
        .filter(|(i, l)| code[*i] && is_server_header(l, name))
        .map(|(i, _)| i)
        .collect()
}

/// Codex: секция `[mcp_servers.<имя>]` в TOML. Разбираем построчно — секция
/// добавляется в конец файла, а при --force заменяется до следующего
/// заголовка `[...]`; остальной конфиг не трогается.
fn install_toml(path: &Path, a: &InstallArgs, entry: &ServerEntry) -> Result<bool, AppError> {
    let existed = path.exists();
    let stamp = file_stamp(path);
    let text = if existed {
        std::fs::read_to_string(path).map_err(AppError::Io)?
    } else {
        String::new()
    };
    let header = format!("[mcp_servers.{}]", toml_key(&a.name));
    let lines: Vec<&str> = text.lines().collect();
    let code = code_lines(&text);
    let start = server_header_lines(&text, &a.name).first().copied();

    let block = entry.toml(&a.name);
    let out = match start {
        Some(i) => {
            if !a.force {
                return Err(AppError::Msg(format!(
                    "в {} уже есть секция {header} — перезаписать: --force",
                    path.display()
                )));
            }
            println!("перезаписываю существующую секцию (--force)");
            // конец секции — следующий заголовок, но только настоящий: `[` в
            // теле многострочного литерала обрезало бы замену не там
            let end = lines
                .iter()
                .enumerate()
                .skip(i + 1)
                .find(|(k, l)| code[*k] && l.trim_start().starts_with('['))
                .map(|(k, _)| k)
                .unwrap_or(lines.len());
            let mut out = String::new();
            for l in &lines[..i] {
                out.push_str(l);
                out.push('\n');
            }
            out.push_str(&block);
            for l in &lines[end..] {
                out.push_str(l);
                out.push('\n');
            }
            out
        }
        None => {
            let mut out = text.clone();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&block);
            out
        }
    };

    ensure_unchanged(path, stamp)?;
    let backup = backup_file(path, a.no_backup || !existed)?;
    write_config(path, out.as_bytes())?;

    // TOML-парсера в зависимостях нет, поэтому проверка простая: секция на
    // месте и ровно одна
    let check = std::fs::read_to_string(path).map_err(AppError::Io)?;
    if server_header_lines(&check, &a.name).len() != 1 {
        restore(path, backup.as_deref(), existed)?;
        return Err(verify_failed(path, backup.as_deref(), existed));
    }
    println!("(TOML не валидируется целиком — проверь `codex mcp list`)");
    Ok(true)
}

/// Отпечаток файла (mtime + размер) на момент чтения; None — файла нет.
fn file_stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Не изменился ли конфиг между нашими read и write.
///
/// Мы делаем read-modify-write чужого файла, а клиент — свой процесс со своим
/// состоянием в памяти: Claude Code, например, переписывает `~/.claude.json`
/// целиком. Гонку с ним не выиграть (после нашей записи он всё равно может
/// записать свою копию), но писать поверх чужой правки и молча её терять мы не
/// будем — окно между read и write закрывается этой проверкой.
fn ensure_unchanged(
    path: &Path,
    before: Option<(std::time::SystemTime, u64)>,
) -> Result<(), AppError> {
    if file_stamp(path) == before {
        return Ok(());
    }
    Err(AppError::Msg(format!(
        "{} изменился, пока мы его читали, — похоже, клиент запущен и пишет конфиг сам; \
         закрой клиента и повтори",
        path.display()
    )))
}

/// Копия конфига рядом с оригиналом. `skip` — файла нет или запрошен
/// --no-backup.
///
/// Занятое имя не перезаписываем, а берём следующее: второй `mcp install
/// --force` затирал бы `.bak` от первого — то есть единственную копию конфига,
/// каким он был до нас.
fn backup_file(path: &Path, skip: bool) -> Result<Option<PathBuf>, AppError> {
    if skip || !path.exists() {
        return Ok(None);
    }
    let backup = (0..100)
        .map(|n| {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(if n == 0 {
                ".bak".to_string()
            } else {
                format!(".bak.{n}")
            });
            path.with_file_name(name)
        })
        .find(|candidate| !candidate.exists())
        .ok_or_else(|| {
            AppError::Msg(format!(
                "рядом с {} уже сто копий .bak — убери лишние",
                path.display()
            ))
        })?;
    std::fs::copy(path, &backup).map_err(AppError::Io)?;
    println!("бэкап: {}", backup.display());
    Ok(Some(backup))
}

/// Проверка перечитанного конфига не прошла: что именно стало с файлом,
/// зависит от того, был ли бэкап.
fn verify_failed(path: &Path, backup: Option<&Path>, existed: bool) -> AppError {
    AppError::Msg(format!(
        "проверка после записи не прошла — {} {}",
        path.display(),
        if backup.is_some() || !existed {
            "восстановлен"
        } else {
            "оставлен как есть (был --no-backup)"
        }
    ))
}

/// Откат неудачной записи. Без бэкапа откатить можно только создание файла:
/// удалять чужой конфиг, которого мы просто не копировали (--no-backup), —
/// хуже самой ошибки.
fn restore(path: &Path, backup: Option<&Path>, existed: bool) -> Result<(), AppError> {
    match backup {
        Some(b) => std::fs::copy(b, path).map(|_| ()).map_err(AppError::Io),
        None if !existed => std::fs::remove_file(path).map_err(AppError::Io),
        None => Ok(()),
    }
}

/// Атомарная запись (tmp + rename, как в store): половинчатый конфиг клиента
/// не должен пережить падение посреди записи. Права существующего файла
/// сохраняем — write_atomic создаёт tmp с 0600, а это не наш файл. Симлинк
/// (конфиг из dotfiles-репо) остаётся симлинком: write_atomic резолвит цель
/// перед переименованием.
fn write_config(path: &Path, data: &[u8]) -> Result<(), AppError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(AppError::Io)?;
    }
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).ok().map(|m| m.permissions().mode())
    };
    fsio::write_atomic(path, data).map_err(AppError::Io)?;
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    Ok(())
}

/// Совпадают ли команда и аргументы (прочие поля записи — пользовательские,
/// их наличие не повод перезаписывать).
fn same_entry(old: &Value, new: &Value) -> bool {
    old.get("command") == new.get("command") && old.get("args") == new.get("args")
}

pub fn status(a: StatusArgs) -> Result<ExitCode, AppError> {
    print_status_table(&clients(), &a.name, a.fmt.pick())
}

fn print_status_table(
    specs: &[ClientSpec],
    name: &str,
    fmt: output::Format,
) -> Result<ExitCode, AppError> {
    let rows: Vec<Vec<Option<String>>> = specs
        .iter()
        .map(|spec| {
            let (state, detail) = match spec.path() {
                Ok(p) => {
                    let (state, detail) = client_status(spec, &p, name);
                    (state.as_str().to_string(), detail)
                }
                Err(e) => ("ошибка".to_string(), e.to_string()),
            };
            vec![Some(spec.key.to_string()), Some(state), Some(detail)]
        })
        .collect();
    // заголовки колонок английские, как у остальных таблиц CLI: они же —
    // ключи в --json
    output::print_rows(&["client", "state", "detail"], &rows, fmt);
    Ok(ExitCode::SUCCESS)
}

/// Состояние клиента — типом, а не строкой: по нему принимает решение шаг
/// `init`, а строка нужна только для таблицы.
#[derive(PartialEq)]
enum ClientState {
    NoFile,
    Unreadable,
    NotParsed,
    Absent,
    Installed,
}

impl ClientState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::NoFile => "нет файла",
            Self::Unreadable => "нечитаем",
            Self::NotParsed => "не разобран",
            Self::Absent => "не настроен",
            Self::Installed => "настроен",
        }
    }
}

/// Клиент, конфиг которого есть на диске.
pub struct PresentClient {
    pub key: &'static str,
    pub title: &'static str,
    pub configured: bool,
}

/// Клиенты с существующим конфигом. Нужно шагу `init`: `mcp install` без
/// аргумента только печатает список, поэтому шаг обязан сам выбрать, кому
/// прописывать сервер.
pub fn present_clients(name: &str) -> Vec<PresentClient> {
    clients()
        .into_iter()
        .filter_map(|spec| {
            let path = spec.path().ok()?;
            let (state, _) = client_status(&spec, &path, name);
            (state != ClientState::NoFile).then_some(PresentClient {
                key: spec.key,
                title: spec.title,
                configured: state == ClientState::Installed,
            })
        })
        .collect()
}

/// Имя записи по умолчанию — то же, что подставляет `mcp install`.
pub fn default_name() -> &'static str {
    DEFAULT_NAME
}

/// Значение ключа `command` в строке TOML, если это именно он.
///
/// За именем ключа обязан идти `=` (после пробелов): без этой проверки
/// `command_timeout = 5` считался ключом `command`, и в колонке detail у `mcp
/// status` вместо команды оказывалось `_timeout = 5`.
fn toml_command_value(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("command")?;
    let value = rest.trim_start().strip_prefix('=')?;
    Some(value.trim().to_string())
}

/// Статус одного клиента: есть ли файл, есть ли в нём наша запись и какой
/// командой она запускается.
fn client_status(spec: &ClientSpec, path: &Path, name: &str) -> (ClientState, String) {
    if !path.exists() {
        return (ClientState::NoFile, path.display().to_string());
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return (ClientState::Unreadable, e.to_string()),
    };
    if spec.layout == Layout::CodexToml {
        let Some(start) = server_header_lines(&text, name).first().copied() else {
            return (ClientState::Absent, path.display().to_string());
        };
        let code = code_lines(&text);
        let command = text
            .lines()
            .enumerate()
            .skip(start + 1)
            .take_while(|(i, l)| !(code[*i] && l.trim_start().starts_with('[')))
            .find_map(|(_, l)| toml_command_value(l))
            .unwrap_or_default();
        return (
            ClientState::Installed,
            command.trim_matches('"').to_string(),
        );
    }
    let root: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return (ClientState::NotParsed, format!("{e}")),
    };
    match root.get(spec.wrapper()).and_then(|s| s.get(name)) {
        Some(entry) => {
            let command = entry.get("command").and_then(Value::as_str).unwrap_or("?");
            let args = entry
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            (
                ClientState::Installed,
                format!("{command} {args}").trim().to_string(),
            )
        }
        None => (ClientState::Absent, path.display().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> ServerEntry {
        ServerEntry {
            command: "/opt/sql-kai".to_string(),
            args: vec!["mcp".to_string()],
        }
    }

    /// Установка правит файлы клиентов, поэтому тесты гоняют её на реальном
    /// файле во временном каталоге — но по явному пути, а не через $HOME.
    fn tmp_path(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("sql-kai-mcp-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut bak = p.file_name().unwrap().to_os_string();
        bak.push(".bak");
        let _ = std::fs::remove_file(p.with_file_name(bak));
        p
    }

    fn install_args(name: &str, force: bool) -> InstallArgs {
        InstallArgs {
            client: None,
            profile: None,
            name: name.to_string(),
            command: None,
            no_backup: false,
            dry_run: false,
            force,
        }
    }

    fn spec(layout: Layout) -> ClientSpec {
        ClientSpec {
            key: "test",
            title: "Test",
            layout,
            rel_path: "unused",
        }
    }

    #[test]
    fn json_install_merges_and_backs_up() {
        let path = tmp_path("merge.json");
        std::fs::write(
            &path,
            r#"{"numStartups":12,"mcpServers":{"other":{"command":"/usr/bin/other","args":["x"]}}}"#,
        )
        .unwrap();
        let spec = spec(Layout::McpServers);
        let a = install_args("sql-kai", false);
        assert!(install_json(&spec, &path, &a, &entry()).unwrap());

        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // чужие ключи и чужой сервер на месте
        assert_eq!(root["numStartups"], 12);
        assert_eq!(root["mcpServers"]["other"]["command"], "/usr/bin/other");
        assert_eq!(root["mcpServers"]["sql-kai"]["command"], "/opt/sql-kai");
        let mut bak = path.file_name().unwrap().to_os_string();
        bak.push(".bak");
        assert!(path.with_file_name(bak).exists(), "нет бэкапа");

        // повторная установка ничего не меняет
        assert!(!install_json(&spec, &path, &a, &entry()).unwrap());
        // другая команда без --force — ошибка
        let other = ServerEntry {
            command: "/usr/local/bin/sql-kai".to_string(),
            args: vec!["mcp".to_string()],
        };
        assert!(install_json(&spec, &path, &a, &other).is_err());
        assert!(install_json(&spec, &path, &install_args("sql-kai", true), &other).unwrap());
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            root["mcpServers"]["sql-kai"]["command"],
            "/usr/local/bin/sql-kai"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn toml_install_appends_section_and_replaces_only_its_own() {
        let path = tmp_path("config.toml");
        std::fs::write(
            &path,
            "model = \"gpt-5\"\n\n[mcp_servers.other]\ncommand = \"/usr/bin/other\"\n",
        )
        .unwrap();
        let a = install_args("sql-kai", false);
        assert!(install_toml(&path, &a, &entry()).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("model = \"gpt-5\""));
        assert!(text.contains("[mcp_servers.other]"));
        assert!(
            text.contains("[mcp_servers.sql-kai]\ncommand = \"/opt/sql-kai\"\nargs = [\"mcp\"]\n")
        );

        assert!(install_toml(&path, &a, &entry()).is_err()); // без --force
        let other = ServerEntry {
            command: "/usr/local/bin/sql-kai".to_string(),
            args: vec!["mcp".to_string(), "prod".to_string()],
        };
        assert!(install_toml(&path, &install_args("sql-kai", true), &other).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("[mcp_servers.sql-kai]").count(), 1);
        assert!(text.contains("args = [\"mcp\", \"prod\"]"));
        assert!(text.contains("[mcp_servers.other]"));
        let _ = std::fs::remove_file(&path);
    }

    /// Квотированный ключ — то же имя: иначе рядом появлялась вторая секция, и
    /// codex получал невалидный TOML (дубль таблицы).
    #[test]
    fn toml_install_recognizes_a_quoted_key() {
        assert!(is_server_header("[mcp_servers.sql-kai]", "sql-kai"));
        assert!(is_server_header("[mcp_servers.\"sql-kai\"]", "sql-kai"));
        assert!(is_server_header(
            "  [mcp_servers . \"sql-kai\"]  ",
            "sql-kai"
        ));
        assert!(!is_server_header("[mcp_servers.other]", "sql-kai"));
        assert!(!is_server_header("[mcp_servers_extra.sql-kai]", "sql-kai"));
        assert!(!is_server_header("command = \"x\"", "sql-kai"));

        let path = tmp_path("quoted.toml");
        std::fs::write(&path, "[mcp_servers.\"sql-kai\"]\ncommand = \"/old\"\n").unwrap();
        // без --force это уже настроенный сервер, а не пустое место
        assert!(install_toml(&path, &install_args("sql-kai", false), &entry()).is_err());
        assert!(install_toml(&path, &install_args("sql-kai", true), &entry()).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("[mcp_servers.").count(), 1);
        assert!(!text.contains("/old"));
        let _ = std::fs::remove_file(&path);
    }

    /// serde_json собран с preserve_order: чужой конфиг возвращается с тем же
    /// порядком ключей. Иначе `mcp install` пересортировывал весь файл по
    /// алфавиту — данные целы, но пользователь видит diff во весь конфиг.
    #[test]
    fn json_install_keeps_the_original_key_order() {
        let path = tmp_path("order.json");
        std::fs::write(
            &path,
            r#"{"zeta":1,"alpha":2,"mcpServers":{"zzz":{"command":"/z","args":[]}},"beta":3}"#,
        )
        .unwrap();
        let a = install_args("sql-kai", false);
        assert!(install_json(&spec(Layout::McpServers), &path, &a, &entry()).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        let at = |k: &str| {
            text.find(k)
                .unwrap_or_else(|| panic!("нет ключа {k}:\n{text}"))
        };
        assert!(
            at("\"zeta\"") < at("\"alpha\"")
                && at("\"alpha\"") < at("\"mcpServers\"")
                && at("\"mcpServers\"") < at("\"beta\""),
            "порядок ключей переписан:\n{text}"
        );
        // наша запись дописана к чужим, а не вставлена перед ними
        assert!(at("\"zzz\"") < at("\"sql-kai\""), "{text}");
        let _ = std::fs::remove_file(&path);
    }

    /// Многострочный литерал — это данные, а не разметка: заголовок секции
    /// внутри `"""…"""` не должен ни требовать --force, ни быть вырезанным.
    #[test]
    fn toml_scan_skips_multiline_literals() {
        let text = "notice = \"\"\"\n[mcp_servers.sql-kai]\ncommand = \"/evil\"\n\"\"\"\n\
                    \n[mcp_servers.other]\ncommand = \"/usr/bin/other\"\n";
        assert!(server_header_lines(text, "sql-kai").is_empty());
        assert_eq!(server_header_lines(text, "other").len(), 1);
        // '''…''' — то же самое, а после закрытия скан снова видит разметку
        let lit = "x = '''\n[mcp_servers.sql-kai]\n'''\n[mcp_servers.sql-kai]\n";
        assert_eq!(server_header_lines(lit, "sql-kai"), vec![3]);

        let path = tmp_path("multiline.toml");
        std::fs::write(&path, text).unwrap();
        // секции нет — установка проходит без --force и литерал остаётся цел
        assert!(install_toml(&path, &install_args("sql-kai", false), &entry()).unwrap());
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(
            out.contains("command = \"/evil\""),
            "литерал испорчен: {out}"
        );
        assert!(out.contains("[mcp_servers.other]"));
        assert!(
            out.ends_with("[mcp_servers.sql-kai]\ncommand = \"/opt/sql-kai\"\nargs = [\"mcp\"]\n")
        );
        let _ = std::fs::remove_file(&path);
    }

    /// `command_timeout = 5` — не ключ `command`: без проверки разделителя в
    /// detail у `mcp status` уезжало `_timeout = 5`.
    #[test]
    fn toml_command_value_needs_the_separator() {
        assert_eq!(
            toml_command_value("command = \"/opt/sql-kai\""),
            Some("\"/opt/sql-kai\"".into())
        );
        assert_eq!(
            toml_command_value("  command=\"/x\""),
            Some("\"/x\"".into())
        );
        assert_eq!(toml_command_value("command_timeout = 5"), None);
        assert_eq!(toml_command_value("args = [\"mcp\"]"), None);
    }

    /// Конфиг подменили между чтением и записью (запущенный клиент сохранил
    /// свою копию) — молча затирать его нельзя.
    #[test]
    fn install_refuses_a_config_changed_under_us() {
        let path = tmp_path("racy.json");
        std::fs::write(&path, "{}").unwrap();
        let stamp = file_stamp(&path);
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, "{\"numStartups\":1}").unwrap();
        assert!(ensure_unchanged(&path, stamp).is_err());
        assert!(ensure_unchanged(&path, file_stamp(&path)).is_ok());
        // файла не было и нет — тоже «не менялся»
        let _ = std::fs::remove_file(&path);
        assert!(ensure_unchanged(&path, None).is_ok());
    }

    #[test]
    fn json_install_refuses_broken_config() {
        let path = tmp_path("broken.json");
        std::fs::write(&path, "{ /* jsonc */ }").unwrap();
        let err = install_json(
            &spec(Layout::McpServers),
            &path,
            &install_args("sql-kai", false),
            &entry(),
        );
        assert!(err.is_err());
        // файл не тронут
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ /* jsonc */ }");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn vscode_entry_declares_stdio() {
        let v = entry().json(Layout::VsCodeServers);
        assert_eq!(v["type"], "stdio");
        assert_eq!(v["command"], "/opt/sql-kai");
        let plain = entry().json(Layout::McpServers);
        assert!(plain.get("type").is_none());
    }

    #[test]
    fn toml_block_quotes_key_when_needed() {
        assert!(entry()
            .toml("sql-kai")
            .starts_with("[mcp_servers.sql-kai]\n"));
        assert!(entry()
            .toml("sql kai")
            .starts_with("[mcp_servers.\"sql kai\"]\n"));
        assert!(entry().toml("sql-kai").contains("args = [\"mcp\"]"));
    }

    #[test]
    fn same_entry_ignores_extra_keys() {
        let new = entry().json(Layout::McpServers);
        let mut old = new.clone();
        old["env"] = json!({ "FOO": "1" });
        assert!(same_entry(&old, &new));
        old["args"] = json!(["mcp", "prod"]);
        assert!(!same_entry(&old, &new));
    }

    #[test]
    fn every_client_key_is_unique() {
        let specs = clients();
        let mut keys: Vec<&str> = specs.iter().map(|c| c.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len());
    }
}
