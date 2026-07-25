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

/// Codex: секция `[mcp_servers.<имя>]` в TOML. Разбираем построчно — секция
/// добавляется в конец файла, а при --force заменяется до следующего
/// заголовка `[...]`; остальной конфиг не трогается.
fn install_toml(path: &Path, a: &InstallArgs, entry: &ServerEntry) -> Result<bool, AppError> {
    let existed = path.exists();
    let text = if existed {
        std::fs::read_to_string(path).map_err(AppError::Io)?
    } else {
        String::new()
    };
    let header = format!("[mcp_servers.{}]", toml_key(&a.name));
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.trim() == header);

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
            let end = lines
                .iter()
                .skip(i + 1)
                .position(|l| l.trim_start().starts_with('['))
                .map(|k| i + 1 + k)
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

    let backup = backup_file(path, a.no_backup || !existed)?;
    write_config(path, out.as_bytes())?;

    // TOML-парсера в зависимостях нет, поэтому проверка простая: секция на
    // месте и ровно одна
    let check = std::fs::read_to_string(path).map_err(AppError::Io)?;
    if check.lines().filter(|l| l.trim() == header).count() != 1 {
        restore(path, backup.as_deref(), existed)?;
        return Err(verify_failed(path, backup.as_deref(), existed));
    }
    println!("(TOML не валидируется целиком — проверь `codex mcp list`)");
    Ok(true)
}

/// Копия конфига рядом с оригиналом. `skip` — файла нет или запрошен
/// --no-backup.
fn backup_file(path: &Path, skip: bool) -> Result<Option<PathBuf>, AppError> {
    if skip || !path.exists() {
        return Ok(None);
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    let backup = path.with_file_name(name);
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
/// сохраняем — write_atomic создаёт tmp с 0600, а это не наш файл.
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
                Ok(p) => client_status(spec, &p, name),
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

/// Статус одного клиента: есть ли файл, есть ли в нём наша запись и какой
/// командой она запускается.
fn client_status(spec: &ClientSpec, path: &Path, name: &str) -> (String, String) {
    if !path.exists() {
        return ("нет файла".into(), path.display().to_string());
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return ("нечитаем".into(), e.to_string()),
    };
    if spec.layout == Layout::CodexToml {
        let header = format!("[mcp_servers.{}]", toml_key(name));
        let mut lines = text.lines().skip_while(|l| l.trim() != header);
        if lines.next().is_none() {
            return ("не настроен".into(), path.display().to_string());
        }
        let command = lines
            .take_while(|l| !l.trim_start().starts_with('['))
            .find_map(|l| l.trim().strip_prefix("command").map(|v| v.trim_start_matches([' ', '=']).trim().to_string()))
            .unwrap_or_default();
        return ("настроен".into(), command.trim_matches('"').to_string());
    }
    let root: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return ("не разобран".into(), format!("{e}")),
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
            ("настроен".into(), format!("{command} {args}").trim().to_string())
        }
        None => ("не настроен".into(), path.display().to_string()),
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

        let root: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
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
        let root: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["mcpServers"]["sql-kai"]["command"], "/usr/local/bin/sql-kai");
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
        assert!(text.contains("[mcp_servers.sql-kai]\ncommand = \"/opt/sql-kai\"\nargs = [\"mcp\"]\n"));

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

    #[test]
    fn json_install_refuses_broken_config() {
        let path = tmp_path("broken.json");
        std::fs::write(&path, "{ /* jsonc */ }").unwrap();
        let err = install_json(&spec(Layout::McpServers), &path, &install_args("sql-kai", false), &entry());
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
        assert!(entry().toml("sql-kai").starts_with("[mcp_servers.sql-kai]\n"));
        assert!(entry().toml("sql kai").starts_with("[mcp_servers.\"sql kai\"]\n"));
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
