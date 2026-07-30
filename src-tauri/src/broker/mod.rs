//! Локальный брокер сессий: unix-сокет, через который sql-kai работает с базами
//! силами GUI-процесса (vault уже разблокирован, туннели уже подняты).
//!
//! Модуль намеренно не знает про Tauri — хост отдаёт ему [`BrokerHooks`]
//! (снапшот GUI-сессий + колбэк «состав cli-сессий изменился»), поэтому тот же
//! сервер можно поселить в отдельный демон, не меняя протокол и клиентов.
//!
//! Протокол: JSON-line на запрос, JSON-line на ответ.
//!   → {"id":1,"method":"query","params":{"profileId":"…","sql":"…"}}
//!   ← {"id":1,"result":{…}} | {"id":1,"error":"…","code":"vault_locked","sqlstate":null}
//!
//! Раскладка: protocol.rs — wire-типы запросов/ответов, state.rs — владелец
//! cli-сессий и их TTL, server.rs — цикл сокета и диспетчер методов
//! (unix-only). Здесь — пути/бинд сокетов и хост-хуки.

mod protocol;
#[cfg(unix)]
mod server;
mod state;

pub use protocol::{
    BrokerSessionInfo, GuiOpen, HelloReply, Method, Request, WireColumnTypes, PROTOCOL_VERSION,
};
#[cfg(unix)]
pub use server::serve;
pub use state::{BrokerState, CliEntry};

use std::path::PathBuf;

#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

use crate::error::AppError;
use crate::fsio;

pub fn socket_path() -> Result<PathBuf, AppError> {
    fsio::config_path("broker.sock")
}

/// Сокет holder'а — фонового держателя cli-сессий на то время, когда GUI не
/// запущен (`sql-kai holder run`). Тот же протокол, что и у GUI-брокера.
pub fn holder_socket_path() -> Result<PathBuf, AppError> {
    fsio::config_path("holder.sock")
}

/// Бинд сокета holder'а: если по нему уже кто-то отвечает — занято (None,
/// живой holder обслужит клиентов сам), иначе протухший файл убирается и
/// поднимается свежий 0600-листенер. Гонка двух спавнеров решается самим
/// бинд-фактом: проигравший увидит живой сокет и выйдет.
#[cfg(unix)]
pub async fn bind_holder() -> Result<Option<UnixListener>, AppError> {
    let path = holder_socket_path()?;
    if UnixStream::connect(&path).await.is_ok() {
        return Ok(None);
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        // Как и у GUI-брокера: не ужали права — не поднимаемся, через сокет
        // выполняется SQL под разлоченным vault.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(Some(listener))
}

/// Best-effort «погасни» в сокет holder'а. Vault lock означает «ничего не
/// остаётся подключенным» — включая фоновый держатель cli-сессий.
#[cfg(unix)]
pub async fn shutdown_holder() {
    let Ok(path) = holder_socket_path() else { return };
    let Ok(stream) = UnixStream::connect(&path).await else { return };
    let (read, mut write) = stream.into_split();
    if write
        .write_all(b"{\"id\":1,\"method\":\"shutdown\",\"params\":null}\n")
        .await
        .is_err()
    {
        return;
    }
    // дождаться ответа (не дольше секунды), чтобы holder успел принять запрос
    let mut lines = BufReader::new(read).lines();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), lines.next_line()).await;
}

/// Removes a stale socket file and binds a fresh 0600 listener.
///
/// В отличие от [`bind_holder`] живой сокет не проверяется — второй GUI-процесс
/// молча перехватит сокет первого, и sql-kai будет ходить сессиями второго.
/// Расчёт на то, что двух GUI не бывает (LaunchServices активирует уже
/// запущенный .app), а не на проверку: dev-сборка рядом с установленной этот
/// расчёт нарушает. Понадобится параллельный запуск — сюда нужен тот же
/// `UnixStream::connect`, что в `bind_holder`.
#[cfg(unix)]
pub fn bind() -> Result<UnixListener, AppError> {
    let path = socket_path()?;
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        // Не сумели ужать права — сокет не поднимаем: через него выполняется
        // SQL под разлоченным vault.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(listener)
}

/// Future-хук «спроси GUI»: profile_id → payload ответа webview (или текст
/// ошибки). Ответ приезжает асинхронно (event → invoke), отсюда future.
pub type GuiSelectionHook = Box<
    dyn Fn(
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
        > + Send
        + Sync,
>;

/// Хост-специфика: как посмотреть GUI-сессии и как сообщить интерфейсу об
/// изменениях. В Tauri это AppState-снапшот и emit события; в будущем демоне —
/// пустой список и no-op.
pub struct BrokerHooks {
    pub gui_sessions: Box<dyn Fn() -> Vec<BrokerSessionInfo> + Send + Sync>,
    pub changed: Box<dyn Fn() + Send + Sync>,
    /// sql-kai сообщил об изменении профилей — интерфейс перечитывает список.
    pub profiles_changed: Box<dyn Fn() + Send + Sync>,
    /// Обработчик `shutdown`: holder закрывает сессии и завершается.
    /// None (GUI-брокер) — метод отвергается: приложение так не гасят.
    pub shutdown: Option<Box<dyn Fn() + Send + Sync>>,
    /// Открыть вкладку в GUI (методы open_table/open_query от MCP-tools).
    /// None (holder) — метод отвергается: интерфейса нет.
    pub open_in_gui: Option<Box<dyn Fn(GuiOpen) + Send + Sync>>,
    /// Текущая вкладка/выделение в GUI (метод gui_selection от MCP-tool
    /// `selection`). None (holder) — метод отвергается: интерфейса нет.
    pub gui_selection: Option<GuiSelectionHook>,
}
