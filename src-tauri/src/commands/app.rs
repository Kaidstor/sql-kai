//! App-level odds and ends: concealed clipboard, the agent GUI-reply channel,
//! CLI install/discovery and the update relaunch.

use tauri::State;

use crate::error::AppError;

use super::AppState;

/// Copies text marked as concealed (`org.nspasteboard.ConcealedType` on macOS)
/// so clipboard-history managers skip it.
#[tauri::command]
pub fn copy_text_concealed(text: String) -> Result<(), AppError> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| AppError::Msg(e.to_string()))?;
    let set = clipboard.set();
    #[cfg(target_os = "macos")]
    let set = arboard::SetExtApple::exclude_from_history(set);
    set.text(text).map_err(|e| AppError::Msg(e.to_string()))
}

/// Ответ webview на событие `agent://gui-request` (метод gui_selection
/// брокера, MCP-tool `selection`). Опоздавший ответ (хук уже снял отправителя
/// по таймауту) молча игнорируется.
#[tauri::command]
pub fn agent_gui_reply(state: State<AppState>, id: String, payload: serde_json::Value) {
    if let Some(tx) = state.gui_requests.lock().unwrap().remove(&id) {
        let _ = tx.send(payload);
    }
}

/// Абсолютный путь к бандл-CLI (`sql-kai-cli` рядом с бинарём приложения) —
/// команда MCP-сервера, который панель агента передаёт в ACP session/new.
/// None — сборка без sidecar (dev до первого cargo build --bins).
#[tauri::command]
pub fn cli_bin_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let src = exe.parent()?.join("sql-kai-cli");
    src.exists().then(|| src.display().to_string())
}

/// Вместе с CLI раскладывается и агентский скилл — Install CLI и есть
/// «подключить sql-kai агентам». Бест-эффорт: симлинки скилла не трогаются,
/// ошибки не валят установку самого CLI.
#[cfg(target_os = "macos")]
fn install_skills() {
    for (_, dir) in crate::skills_sync::agent_dirs() {
        let _ = crate::skills_sync::install(&dir, false);
    }
}

/// Installs the `sql-kai` CLI into the system PATH, "big-company" style
/// (Zed / VS Code): symlinks the `sql-kai-cli` sidecar bundled next to the
/// running app into `/usr/local/bin` — which is always on PATH via
/// `/etc/paths`. The symlink points into the .app, so future app updates
/// carry the CLI along. Tries a direct symlink first (writable Homebrew
/// setups) and falls back to an admin prompt (password / Touch ID) when the
/// dir is root-owned. Returns the created path; the sentinel error
/// `"cancelled"` means the user dismissed the auth dialog.
#[tauri::command]
pub fn install_cli() -> Result<String, AppError> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::symlink;
        use std::path::Path;

        let exe = std::env::current_exe()?;
        let src = exe
            .parent()
            .map(|p| p.join("sql-kai-cli"))
            .ok_or_else(|| AppError::Msg("could not resolve the app bundle path".into()))?;
        if !src.exists() {
            return Err(AppError::Msg(format!(
                "CLI-бинарь не найден рядом с приложением: {}\n\
                 Нужна версия sql-kai со встроенным sql-kai-cli (sidecar).",
                src.display()
            )));
        }
        let target = Path::new("/usr/local/bin/sql-kai");

        // Fast path: recreate the symlink directly when /usr/local/bin is
        // writable (e.g. Homebrew) — no password prompt needed.
        let _ = std::fs::remove_file(target); // ignore "not found" / "denied"
        if symlink(&src, target).is_ok() {
            install_skills();
            return Ok(target.display().to_string());
        }

        // Slow path: the dir is root-owned. Escalate via the native auth
        // dialog; `ln -sf` handles a pre-existing root-owned symlink.
        //
        // src/target are passed as osascript `argv` (never interpolated into
        // the script text) and shell-escaped inside AppleScript with `quoted
        // form of` — so a bundle path containing a quote or space can't break
        // out of the privileged shell command.
        const INSTALL_SCRIPT: &str = "on run argv\n\
            do shell script \"mkdir -p /usr/local/bin && ln -sf \" \
            & quoted form of (item 1 of argv) & \" \" \
            & quoted form of (item 2 of argv) with administrator privileges\n\
            end run";
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(INSTALL_SCRIPT)
            .arg(&src)
            .arg(target)
            .output()?;
        if out.status.success() {
            install_skills();
            return Ok(target.display().to_string());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        // -128 == user dismissed the auth dialog.
        if stderr.contains("-128") || stderr.contains("User canceled") {
            return Err(AppError::Msg("cancelled".into()));
        }
        Err(AppError::Msg(format!(
            "could not create the symlink: {}",
            stderr.trim()
        )))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(AppError::Msg(
            "Install CLI is supported on macOS only".into(),
        ))
    }
}

/// Relaunch to apply an update. plugin-process relaunch() spawns the binary
/// directly, bypassing LaunchServices — modern macOS denies activation to such
/// a process, so the new window starts behind others. `open -n` relaunches the
/// bundle as a user-initiated launch and the window comes to front.
#[tauri::command]
pub fn relaunch_app(app: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        // …/sql-kai.app/Contents/MacOS/sql-kai → …/sql-kai.app
        let bundle = std::env::current_exe().ok().and_then(|exe| {
            let b = exe.ancestors().nth(3)?;
            b.extension()
                .is_some_and(|e| e == "app")
                .then(|| b.to_path_buf())
        });
        if let Some(bundle) = bundle {
            let spawned = std::process::Command::new("open")
                .arg("-n")
                .arg(&bundle)
                .spawn()
                .is_ok();
            if spawned {
                app.exit(0);
                return;
            }
        }
    }
    // Outside a bundle (dev run) or non-macOS — plain restart.
    app.restart();
}
