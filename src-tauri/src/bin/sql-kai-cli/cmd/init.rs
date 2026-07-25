//! `sql-kai init` — первичная настройка CLI: симлинк в PATH, тихий доступ к
//! vault, MCP-сервер в агентов, автодополнение. Всё это раньше было
//! инструкцией в README («сделай `ln -sf …` руками»), а инструкцию никто не
//! выполняет целиком.
//!
//! Главное правило команды: она не делает молча ничего, что видно за
//! пределами репозитория. Каждый шаг сначала печатает точный путь, который
//! будет создан или изменён, и только потом спрашивает. Без TTY (пайп, CI,
//! запуск из агента) init не меняет ничего вообще — печатает список ручных
//! команд и выходит успешно.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Args;
use sql_kai_lib::error::AppError;
use sql_kai_lib::{fsio, vault};

use crate::cmd::completion::{self, CompletionShell};
use crate::cmd::mcp_setup;
use crate::cmd::vault::VaultCmd;
use crate::remote::shq;

/// Штатное место установки приложения — запасной источник симлинка, когда
/// сам CLI запущен не из бандла (например, распакован из tar.gz).
const APP_CLI: &str = "/Applications/sql-kai.app/Contents/MacOS/sql-kai-cli";

#[derive(Args)]
pub struct InitArgs {
    /// Каталог из PATH для симлинка (по умолчанию ~/.local/bin)
    #[arg(long, value_name = "DIR")]
    bin_dir: Option<PathBuf>,
    /// Шелл для автодополнения (по умолчанию — из $SHELL)
    #[arg(long, value_enum)]
    shell: Option<CompletionShell>,
    /// Молча выйти, если всё уже настроено
    #[arg(long)]
    skip_if_configured: bool,
}

pub async fn run(a: InitArgs) -> Result<ExitCode, AppError> {
    let shell = a.shell.or_else(completion::detect_shell);
    if a.skip_if_configured && all_configured(&a, shell) {
        return Ok(ExitCode::SUCCESS);
    }
    if !std::io::stdin().is_terminal() {
        print_manual(&a, shell);
        return Ok(ExitCode::SUCCESS);
    }

    println!("sql-kai init — первичная настройка. Ничего не меняю без подтверждения.");

    let mut failed = false;
    println!("\n[1/4] команда sql-kai в PATH");
    failed |= report(step_path(&a));
    println!("\n[2/4] тихий доступ CLI к паролям (vault trust)");
    failed |= report(step_vault().await);
    println!("\n[3/4] MCP-сервер для AI-агентов");
    failed |= report(step_mcp());
    println!("\n[4/4] автодополнение шелла");
    failed |= report(step_completion(shell));

    if failed {
        println!("\nЧасть шагов не выполнена — см. сообщения выше.");
        return Ok(ExitCode::FAILURE);
    }
    println!("\nГотово. Дальше: `sql-kai discover <ssh-alias>` — завести первый профиль.");
    Ok(ExitCode::SUCCESS)
}

/// Ошибка шага не роняет init: шаги независимы, и настройка наполовину
/// лучше, чем прерванная на первом же спотыкании. Возвращает «было плохо».
fn report(result: Result<(), AppError>) -> bool {
    match result {
        Ok(()) => false,
        Err(e) => {
            println!("  ошибка: {e}");
            true
        }
    }
}

// --- шаг 1: симлинк в PATH ----------------------------------------------------

fn step_path(a: &InitArgs) -> Result<(), AppError> {
    let Some(src) = bundle_cli() else {
        let exe = current_exe_display();
        println!("  запущено не из бандла ({exe}) — шаг пропущен.");
        println!(
            "  Симлинк имеет смысл на CLI внутри sql-kai.app ({APP_CLI}):\n  \
             тогда обновление приложения обновляет и команду."
        );
        return Ok(());
    };
    let target = link_target(a)?;
    let dir = target
        .parent()
        .ok_or_else(|| AppError::Msg(format!("у пути {} нет каталога", target.display())))?
        .to_path_buf();

    if std::fs::read_link(&target).is_ok_and(|cur| cur == src) {
        println!("  уже настроено: {} → {}", target.display(), src.display());
        warn_if_not_in_path(&dir);
        return Ok(());
    }

    println!("  симлинк:  {}", target.display());
    println!("  на бинарь: {}", src.display());
    if !dir.exists() {
        println!("  каталог {} будет создан", dir.display());
    }
    match std::fs::read_link(&target) {
        Ok(cur) => println!("  ⚠ существующий симлинк будет заменён (сейчас → {})", cur.display()),
        Err(_) if target.exists() => println!("  ⚠ существующий файл будет заменён"),
        Err(_) => {}
    }
    if !confirm("создать симлинк?")? {
        println!("  пропущено");
        return Ok(());
    }

    std::fs::create_dir_all(&dir)?;
    // remove_file вместо перезаписи: symlink(2) не умеет заменять существующий
    // путь. Ошибка «нет такого файла» здесь ожидаема и игнорируется.
    let _ = std::fs::remove_file(&target);
    std::os::unix::fs::symlink(&src, &target)
        .map_err(|e| AppError::Msg(format!("симлинк {}: {e}", target.display())))?;
    println!("  ✓ {} → {}", target.display(), src.display());
    warn_if_not_in_path(&dir);
    Ok(())
}

/// Бинарь, на который стоит ставить симлинк: тот, что лежит в .app — тогда
/// обновление приложения обновляет и CLI (модель VS Code). `canonicalize`
/// обязателен: запуск через уже существующий симлинк иначе выглядел бы как
/// запуск вне бандла.
fn bundle_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let real = std::fs::canonicalize(&exe).unwrap_or(exe);
    if real
        .ancestors()
        .any(|a| a.extension().is_some_and(|e| e == "app"))
    {
        return Some(real);
    }
    let fallback = Path::new(APP_CLI);
    fallback.exists().then(|| fallback.to_path_buf())
}

fn current_exe_display() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into())
}

fn link_target(a: &InitArgs) -> Result<PathBuf, AppError> {
    let dir = match &a.bin_dir {
        Some(d) => d.clone(),
        None => home()?.join(".local").join("bin"),
    };
    Ok(dir.join("sql-kai"))
}

fn warn_if_not_in_path(dir: &Path) {
    if dir_in_path(dir) {
        return;
    }
    println!("  ⚠ {} не в PATH. Добавь строку в ~/.zshrc:", dir.display());
    println!("      export PATH=\"{}:$PATH\"", dir.display());
}

fn dir_in_path(dir: &Path) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let canon = std::fs::canonicalize(dir).ok();
    std::env::split_paths(&path)
        .any(|p| p == dir || (canon.is_some() && std::fs::canonicalize(&p).ok() == canon))
}

/// Команда `sql-kai` уже доступна из PATH (не важно, кем поставлена).
fn cli_in_path() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|d| d.join("sql-kai").exists())
}

// --- шаг 2: vault trust -------------------------------------------------------

async fn step_vault() -> Result<(), AppError> {
    if !vault::exists() {
        println!("  vault ещё не создан — шаг пропущен.");
        println!("  Создать: `sql-kai vault setup` (или первый запуск GUI), потом `sql-kai vault trust`.");
        return Ok(());
    }
    if vault::cli_trust_enrolled() {
        println!("  уже включён (отозвать — `sql-kai vault revoke`)");
        return Ok(());
    }
    println!(
        "  Без него каждый запрос к профилю с паролем просит мастер-пароль vault.\n  \
         Trust кладёт копию ключа в login keychain — CLI читает пароли молча,\n  \
         отзыв в любой момент: `sql-kai vault revoke`."
    );
    if !confirm("включить cli trust (потребуется мастер-пароль)?")? {
        println!("  пропущено");
        return Ok(());
    }
    // Ровно та же логика, что у `sql-kai vault trust` — вызываем её, а не копию.
    crate::cmd::vault::run(VaultCmd::Trust).await?;
    Ok(())
}

// --- шаг 3: MCP-сервер --------------------------------------------------------

fn step_mcp() -> Result<(), AppError> {
    let exe = std::env::current_exe()?;
    // `mcp install` без имени клиента только печатает таблицу и выходит с нулём:
    // шаг рапортовал об успехе, ничего не прописав. Клиента выбираем здесь — по
    // тем, у кого конфиг реально лежит на диске.
    let present = mcp_setup::present_clients(mcp_setup::default_name());
    if present.is_empty() {
        println!("  Не нашёл конфигов известных MCP-клиентов (claude-code, codex, cursor, …).");
        println!("  Когда появятся: `sql-kai mcp install <клиент>` (список — `sql-kai mcp install`).");
        return Ok(());
    }
    let pending: Vec<&mcp_setup::PresentClient> =
        present.iter().filter(|c| !c.configured).collect();
    if pending.is_empty() {
        let titles: Vec<&str> = present.iter().map(|c| c.title).collect();
        println!("  Уже прописан: {}.", titles.join(", "));
        return Ok(());
    }
    println!(
        "  Пропишет sql-kai как MCP-сервер в конфиги AI-агентов —\n  \
         агент сможет читать схему и выполнять запросы через tools."
    );
    let titles: Vec<&str> = pending.iter().map(|c| c.title).collect();
    println!("  Найдены без записи: {}.", titles.join(", "));
    if !confirm("прописать сервер этим клиентам?")? {
        println!("  пропущено");
        return Ok(());
    }
    // Клиенты обрабатываются независимо (побитый конфиг одного не повод не
    // прописать остальным), но провал каждого обязан дойти до итога init:
    // иначе «Готово» и код 0 при неработающей настройке.
    let mut failed: Vec<&str> = Vec::new();
    for client in pending {
        let mut cmd = Command::new(&exe);
        cmd.args(["mcp", "install", client.key]);
        vault::scrub_master_password_env(&mut cmd);
        let status = cmd.status().map_err(|e| {
            AppError::Msg(format!("не смог запустить {} mcp install: {e}", exe.display()))
        })?;
        if !status.success() {
            println!(
                "  ⚠ {}: `mcp install {}` завершилась с кодом {}",
                client.title,
                client.key,
                status.code().unwrap_or(-1)
            );
            failed.push(client.key);
        }
    }
    if !failed.is_empty() {
        return Err(AppError::Msg(format!(
            "не прописан клиентам: {} (причина — в выводе выше; повтор: `sql-kai mcp install <клиент>`)",
            failed.join(", ")
        )));
    }
    Ok(())
}

// --- шаг 4: автодополнение ----------------------------------------------------

fn step_completion(shell: Option<CompletionShell>) -> Result<(), AppError> {
    let Some(shell) = shell else {
        println!("  не понял шелл из $SHELL — шаг пропущен.");
        println!("  Вручную: `sql-kai completion zsh|bash|fish` (или `sql-kai init --shell zsh`).");
        return Ok(());
    };
    let path = completion_path(shell)?;
    let text = completion::script(shell);

    let same = std::fs::read_to_string(&path).is_ok_and(|cur| cur == text);
    if same {
        println!("  скрипт уже актуален: {}", path.display());
    } else {
        println!("  файл: {}", path.display());
        if path.exists() {
            println!("  ⚠ существующий файл будет перезаписан");
        }
        if !confirm(&format!("записать автодополнение для {}?", shell.as_str()))? {
            println!("  пропущено");
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, &text)
            .map_err(|e| AppError::Msg(format!("запись {}: {e}", path.display())))?;
        println!("  ✓ {}", path.display());
    }

    let Some((rc, line)) = rc_hook(shell, &path)? else {
        println!("  fish подхватит файл сам — перезапусти шелл");
        return Ok(());
    };
    if rc_sources(&rc, &line) {
        println!("  {} уже подключает его", rc.display());
        return Ok(());
    }
    println!("  файл: {} — в конец будет дописано:", rc.display());
    println!("      {line}");
    if !confirm(&format!("дописать в {}?", rc.display()))? {
        println!("  пропущено; подключи сам строкой выше");
        return Ok(());
    }
    let block = format!("\n# автодополнение sql-kai (добавлено `sql-kai init`)\n{line}\n");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rc)
        .and_then(|mut f| f.write_all(block.as_bytes()))
        .map_err(|e| AppError::Msg(format!("запись {}: {e}", rc.display())))?;
    println!("  ✓ {} обновлён — перезапусти шелл", rc.display());
    Ok(())
}

/// Куда кладём скрипт. fish сам подхватывает свой completions-каталог,
/// zsh/bash — нет, поэтому их скрипт живёт в конфиге sql-kai (рядом с
/// profiles.json, уважает SQL_KAI_CONFIG_DIR) и подключается строкой в rc.
/// Source вместо fpath — потому что так скрипт гарантированно отработает
/// целиком, включая дополнение профилей.
fn completion_path(shell: CompletionShell) -> Result<PathBuf, AppError> {
    match shell {
        CompletionShell::Fish => Ok(home()?
            .join(".config")
            .join("fish")
            .join("completions")
            .join(completion::file_name(shell))),
        _ => fsio::config_path(completion::file_name(shell)),
    }
}

/// rc-файл и строка подключения; None — шеллу rc не нужен (fish).
fn rc_hook(shell: CompletionShell, path: &Path) -> Result<Option<(PathBuf, String)>, AppError> {
    let home = home()?;
    let rc = match shell {
        CompletionShell::Zsh => home.join(".zshrc"),
        // На macOS Terminal запускает bash как login-шелл и читает
        // ~/.bash_profile, а ~/.bashrc — нет. Берём тот файл, который у
        // пользователя уже есть, иначе строка ушла бы в никуда.
        CompletionShell::Bash => {
            let rc = home.join(".bashrc");
            let profile = home.join(".bash_profile");
            if !rc.exists() && profile.exists() {
                profile
            } else {
                rc
            }
        }
        CompletionShell::Fish => return Ok(None),
    };
    let line = format!("source {}", shq(&path.to_string_lossy()));
    Ok(Some((rc, line)))
}

fn rc_sources(rc: &Path, line: &str) -> bool {
    std::fs::read_to_string(rc).is_ok_and(|text| text.contains(line))
}

// --- общее --------------------------------------------------------------------

fn home() -> Result<PathBuf, AppError> {
    dirs::home_dir().ok_or_else(|| AppError::Msg("не нашёл домашний каталог".into()))
}

/// Вопрос перед изменением на диске. Печатается в stderr, чтобы вывод шагов
/// оставался читаемым при `sql-kai init | tee`.
///
/// Ветка `input::confirm` «нет TTY» сюда не доходит: без терминала `run` выходит
/// раньше, напечатав ручные команды, — поэтому подсказка про флаг здесь только
/// для формы вызова, живым текстом её не увидеть.
fn confirm(question: &str) -> Result<bool, AppError> {
    crate::input::confirm(&format!("  {question}"), "--skip-if-configured")
}

/// Всё ли уже настроено (для `--skip-if-configured`). MCP считаем настроенным,
/// если сервер прописан хотя бы одному найденному клиенту (или клиентов нет
/// вовсе): требовать запись во все — значит спрашивать при каждом запуске у
/// того, кто держит sql-kai в одном агенте из трёх.
fn all_configured(a: &InitArgs, shell: Option<CompletionShell>) -> bool {
    let path_ok = match (bundle_cli(), link_target(a)) {
        (Some(src), Ok(target)) => {
            std::fs::read_link(&target).is_ok_and(|cur| cur == src) || cli_in_path()
        }
        // не из бандла — шаг неприменим
        (None, _) => true,
        (_, Err(_)) => false,
    };
    let vault_ok = !vault::exists() || vault::cli_trust_enrolled();
    let completion_ok = match shell {
        None => true,
        Some(shell) => match completion_path(shell) {
            Ok(path) => {
                path.exists()
                    && match rc_hook(shell, &path) {
                        Ok(Some((rc, line))) => rc_sources(&rc, &line),
                        Ok(None) => true,
                        Err(_) => false,
                    }
            }
            Err(_) => false,
        },
    };
    let present = mcp_setup::present_clients(mcp_setup::default_name());
    let mcp_ok = present.is_empty() || present.iter().any(|c| c.configured);
    path_ok && vault_ok && completion_ok && mcp_ok
}

/// Без TTY подтверждать нечего, поэтому init ничего не трогает — только
/// показывает те же шаги в виде команд.
fn print_manual(a: &InitArgs, shell: Option<CompletionShell>) {
    let shell_name = shell.map(|s| s.as_str()).unwrap_or("zsh");
    println!("sql-kai init: нет TTY — ничего не меняю. Запусти в терминале `sql-kai init` или сделай вручную:");
    match (bundle_cli(), link_target(a)) {
        (Some(src), Ok(target)) => {
            println!("  ln -sf {} {}", shq(&src.to_string_lossy()), shq(&target.to_string_lossy()));
        }
        _ => println!("  # симлинк в PATH: нужен sql-kai.app ({APP_CLI})"),
    }
    println!("  sql-kai vault trust");
    println!("  sql-kai mcp install");
    let shell = shell.unwrap_or(CompletionShell::Zsh);
    if let Ok(path) = completion_path(shell) {
        let quoted = shq(&path.to_string_lossy());
        println!("  sql-kai completion {shell_name} > {quoted}");
        if let Ok(Some((rc, line))) = rc_hook(shell, &path) {
            println!("  echo {} >> {}", shq(&line), shq(&rc.to_string_lossy()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Симлинк ставим на бинарь в бандле — иначе обновление приложения не
    /// подхватится и шаг теряет смысл.
    #[test]
    fn bundle_detection_needs_dot_app() {
        let bundled = Path::new("/Applications/sql-kai.app/Contents/MacOS/sql-kai-cli");
        assert!(bundled
            .ancestors()
            .any(|a| a.extension().is_some_and(|e| e == "app")));
        let dev = Path::new("/Users/kai/sql-kai/src-tauri/target/debug/sql-kai-cli");
        assert!(!dev
            .ancestors()
            .any(|a| a.extension().is_some_and(|e| e == "app")));
    }
}
