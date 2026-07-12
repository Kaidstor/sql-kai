//! Открытие соединения по профилю: пароль (env/sec/vault), ssh-mux, read-only.

use sql_kai_lib::db;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, Profile};

use super::pw::{ensure_vault, resolve_override, PwSource};
use super::resolve::resolve_profile;

/// TTL персистентного ssh-мастера (сек): env KAI_SSH_MUX_TTL, иначе 5 минут.
pub fn mux_ttl() -> u32 {
    std::env::var("KAI_SSH_MUX_TTL")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(300)
}

/// Подключение к профилю; без `write` сессия сразу переводится в read-only.
/// `mux` включает переиспользование ssh-туннеля через ControlMaster.
async fn open(
    profile: &Profile,
    password_override: Option<String>,
    write: bool,
    verbose: bool,
    mux: bool,
) -> Result<db::Connected, AppError> {
    let connected = db::connect(
        profile,
        db::ConnectOptions {
            password_override,
            ssh_mux_ttl: if mux { Some(mux_ttl()) } else { None },
            ..Default::default()
        },
    )
    .await?;
    let _ = store::record_last_connected(&profile.id, store::ConnectVia::Cli);
    if !write {
        db::execute(
            &connected.session.client,
            "SET default_transaction_read_only = on",
            1,
        )
        .await?;
    }
    if verbose {
        let via = connected
            .tunnel_port
            .map(|p| format!(" via ssh:{p}"))
            .unwrap_or_default();
        eprintln!(
            "[{} -> {}@{}:{}/{}{} PostgreSQL {} {}]",
            profile.name,
            profile.user,
            profile.host,
            profile.port,
            profile.database,
            via,
            connected.server_version,
            if write { "rw" } else { "ro" }
        );
    }
    Ok(connected)
}

/// Полный путь для команд: резолв алиаса, пароль (env/sec/vault), соединение.
pub async fn open_for(
    alias: &str,
    pw: PwSource<'_>,
    write: bool,
    verbose: bool,
    mux: bool,
) -> Result<(Profile, db::Connected), AppError> {
    let profile = resolve_profile(alias)?;
    let over = resolve_override(&profile, &pw)?;
    ensure_vault(&profile, over.is_some())?;
    let connected = open(&profile, over, write, verbose, mux).await?;
    // живой GUI (запертый vault / кастомный пароль): пусть лаунчер перечитает
    // профили и обновит "last connected"; без GUI — мгновенный no-op
    crate::broker_client::notify_profiles_changed().await;
    Ok((profile, connected))
}
