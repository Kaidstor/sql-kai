// ACP (Agent Client Protocol) transport: spawns an agent process (Claude
// Code, Gemini CLI, …) and shuttles newline-delimited JSON-RPC between its
// stdio and the webview. The protocol itself lives in the frontend
// (lib/acp.ts) — this module only owns the process lifecycle:
//   stdout line  -> event acp://msg     {agentId, line}
//   stderr line  -> event acp://stderr  {agentId, line}
//   process exit -> event acp://exit    {agentId, code}
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

use crate::error::AppError;
use crate::logging;

#[derive(Default)]
pub struct AcpState {
    agents: Mutex<HashMap<String, AgentProc>>,
}

struct AgentProc {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    child: Arc<Mutex<Child>>,
}

impl AcpState {
    /// Kills every running agent (app shutdown).
    pub fn clear(&self) {
        let mut agents = self.agents.lock().unwrap();
        for (_, proc) in agents.drain() {
            let _ = proc.child.lock().unwrap().start_kill();
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpLineEvent<'a> {
    agent_id: &'a str,
    line: &'a str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpExitEvent<'a> {
    agent_id: &'a str,
    code: Option<i32>,
}

/// PATH городской GUI-процесс на macOS получает урезанным (без brew/nvm) —
/// агентские CLI (`npx`, `gemini`, `claude`) в нём не находятся. Берём PATH
/// логин-шелла один раз и подставляем его детям.
fn login_shell_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let fallback = || {
            let cur = std::env::var("PATH").unwrap_or_default();
            let home = dirs::home_dir().unwrap_or_default();
            format!(
                "{cur}:/opt/homebrew/bin:/usr/local/bin:{}/.local/bin",
                home.display()
            )
        };
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        // -l -i: и .zprofile, и .zshrc (nvm/asdf обычно во втором); без tty
        // zsh может ругаться в stderr — игнорируем, важен только stdout.
        let out = std::process::Command::new(&shell)
            .args(["-lic", "printf %s \"$PATH\""])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        match out {
            Ok(o) if o.status.success() && !o.stdout.is_empty() => {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            }
            _ => fallback(),
        }
    })
}

/// Гейт на команду агента: webview через IPC не запускает произвольный бинарь,
/// только известные ACP-пути — `npm` (установка адаптера), бинарь из
/// `<appData>/acp/…` (установленный адаптер), `cursor-agent` (ставится их
/// инсталлером) и кастомная команда из settings.json. Кастом сверяется с
/// файлом, который пишет backend по явному действию в настройках, а не с тем,
/// что прислал вызов. Это защита в глубину при компрометации webview (основной
/// барьер — CSP), а не граница против процессов того же пользователя.
fn spawn_allowed(app: &AppHandle, cmd: &str) -> Result<(), AppError> {
    if cmd == "npm" || cmd == "cursor-agent" {
        return Ok(());
    }
    if let Ok(dir) = app.path().app_data_dir() {
        // canonicalize обеих сторон: `..` в присланном пути проходит голый
        // starts_with (компоненты сравниваются лексически, без нормализации)
        if let (Ok(cmd_real), Ok(acp_real)) = (
            std::fs::canonicalize(cmd),
            std::fs::canonicalize(dir.join("acp")),
        ) {
            if cmd_real.starts_with(&acp_real) {
                return Ok(());
            }
        }
    }
    if let Ok(settings) = crate::store::load_settings() {
        if let Some(custom) = settings.get("agentCustomCmd").and_then(|v| v.as_str()) {
            // тот же разбор, что parseCustomCmd на фронте: первый токен — бинарь
            if custom.split_whitespace().next() == Some(cmd) {
                return Ok(());
            }
        }
    }
    Err(AppError::Msg(format!(
        "`{cmd}` is not an allowed agent command — only built-in ACP adapters \
         and the custom command saved in Settings can be launched"
    )))
}

/// Запускает процесс агента и подписывает его stdio на события webview.
/// `agent_id` выбирает фронт (uuid) — по нему адресуются send/kill/события.
#[tauri::command]
pub async fn acp_spawn(
    app: AppHandle,
    state: State<'_, AcpState>,
    agent_id: String,
    cmd: String,
    args: Vec<String>,
    cwd: String,
) -> Result<(), AppError> {
    spawn_allowed(&app, &cmd)?;
    // повторный spawn под тем же id — сначала убрать старый процесс
    if let Some(old) = state.agents.lock().unwrap().remove(&agent_id) {
        let _ = old.child.lock().unwrap().start_kill();
    }

    std::fs::create_dir_all(&cwd).map_err(|e| AppError::Msg(format!("agent cwd {cwd}: {e}")))?;

    let mut command = Command::new(&cmd);
    command
        .args(&args)
        // ipv4-first для node-детей: npm и часть агентов без happy-eyeballs
        // виснут навечно на сетях с IPv6-блэкхолом (VPN). Env задаётся здесь,
        // а не приходит из webview: произвольный env из IPC — это
        // NODE_OPTIONS=--require и подобные инъекции в разрешённый бинарь.
        .env("NODE_OPTIONS", "--dns-result-order=ipv4first")
        .env("PATH", login_shell_path())
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::vault::scrub_master_password_env_tokio(&mut command);

    let mut child = command
        .spawn()
        .map_err(|e| AppError::Msg(format!("failed to launch `{cmd}`: {e}")))?;

    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let child = Arc::new(Mutex::new(child));
    state.agents.lock().unwrap().insert(
        agent_id.clone(),
        AgentProc {
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            child: child.clone(),
        },
    );
    logging::log("acp", &format!("[{agent_id}] spawned `{cmd}`"));

    // stderr — диагностика адаптера; в лог и на фронт (панель показывает
    // хвост при падении агента)
    let stderr_app = app.clone();
    let stderr_id = agent_id.clone();
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            logging::log("acp", &format!("[{stderr_id}] stderr: {line}"));
            let _ = stderr_app.emit(
                "acp://stderr",
                AcpLineEvent {
                    agent_id: &stderr_id,
                    line: &line,
                },
            );
        }
    });

    // stdout: строки JSON-RPC → события; EOF = процесс умер (или закрыл
    // stdout) → зачистить и сообщить код выхода.
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        // Ok(None) = EOF, Err = read error — both end the stream.
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = app.emit(
                "acp://msg",
                AcpLineEvent {
                    agent_id: &agent_id,
                    line: &line,
                },
            );
        }
        // дать процессу дожать выход, чтобы забрать код (127 = не найден)
        let mut code = None;
        for _ in 0..20 {
            match child.lock().unwrap().try_wait() {
                Ok(Some(status)) => {
                    code = status.code();
                    break;
                }
                Ok(None) => {}
                Err(_) => break,
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if let Some(st) = app.try_state::<AcpState>() {
            st.agents.lock().unwrap().remove(&agent_id);
        }
        logging::log("acp", &format!("[{agent_id}] exited, code {code:?}"));
        let _ = app.emit(
            "acp://exit",
            AcpExitEvent {
                agent_id: &agent_id,
                code,
            },
        );
    });

    Ok(())
}

/// Пишет одну строку JSON-RPC в stdin агента.
#[tauri::command]
pub async fn acp_send(
    state: State<'_, AcpState>,
    agent_id: String,
    line: String,
) -> Result<(), AppError> {
    let stdin = {
        let agents = state.agents.lock().unwrap();
        agents
            .get(&agent_id)
            .ok_or_else(|| AppError::Msg("agent is not running".into()))?
            .stdin
            .clone()
    };
    let mut stdin = stdin.lock().await;
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| AppError::Msg(format!("agent stdin: {e}")))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| AppError::Msg(format!("agent stdin: {e}")))?;
    stdin
        .flush()
        .await
        .map_err(|e| AppError::Msg(format!("agent stdin: {e}")))?;
    Ok(())
}

/// Останавливает агента (новый чат / закрытие панели / смена провайдера).
#[tauri::command]
pub fn acp_kill(state: State<'_, AcpState>, agent_id: String) -> Result<(), AppError> {
    if let Some(proc) = state.agents.lock().unwrap().remove(&agent_id) {
        let _ = proc.child.lock().unwrap().start_kill();
    }
    Ok(())
}

/// Версия npm-пакета, установленного в управляемую папку адаптера
/// (`<dir>/node_modules/<pkg>/package.json`), или None, если его там нет.
/// Фронт сверяет её с пином и решает, нужен ли `npm install`.
#[tauri::command]
pub fn acp_installed_version(dir: String, pkg: String) -> Option<String> {
    let path = std::path::Path::new(&dir)
        .join("node_modules")
        .join(&pkg)
        .join("package.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(json.get("version")?.as_str()?.to_string())
}
