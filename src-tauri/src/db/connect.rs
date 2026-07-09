use std::sync::atomic::AtomicU8;
use std::sync::Arc;
use std::time::Duration;

use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

use super::sqltext::TxStatus;
use crate::error::AppError;
use crate::logging;
use crate::store::{self, Profile};
use crate::tunnel::{self, Tunnel};

pub struct Session {
    pub profile_id: String,
    /// For the diagnostics log — ids alone are unreadable there.
    pub profile_name: String,
    // Kept so a reloaded frontend can re-adopt live sessions (list_sessions).
    pub server_version: String,
    pub tunnel_port: Option<u16>,
    pub client: Arc<Client>,
    pub cancel: tokio_postgres::CancelToken,
    /// Heuristic transaction state (a [`TxStatus`] as u8), advanced after every
    /// `execute` on this connection. Arc so `execute_sql` can update it after
    /// the await without re-locking the session map.
    pub tx: Arc<AtomicU8>,
    /// A per-tab secondary connection (own backend pid / transaction) that
    /// reuses the profile's tunnel — not the profile's primary session.
    pub isolated: bool,
    // Held so the ssh child stays alive; killed on Drop.
    pub _tunnel: Option<Tunnel>,
    conn_task: tokio::task::JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.conn_task.abort();
    }
}

pub struct Connected {
    pub session: Session,
    pub server_version: String,
    pub tunnel_port: Option<u16>,
}

/// Knobs for [`connect`]. Defaults (all None) = pull secrets from the vault,
/// no ssh multiplexing — the GUI's behavior.
#[derive(Default)]
pub struct ConnectOptions {
    /// Use this DB password instead of the vault's (`--password-env`, tests).
    pub password_override: Option<String>,
    /// Use this SSH key passphrase instead of the vault's.
    pub ssh_passphrase_override: Option<String>,
    /// Some(ttl) → reuse ssh auth via a persistent ControlMaster that lingers
    /// `ttl` seconds idle (CLI). None → standalone tunnel (GUI).
    pub ssh_mux_ttl: Option<u32>,
    /// Some((host, port)) → connect straight to this endpoint and open NO tunnel
    /// of our own (a secondary/isolated connection reusing the primary session's
    /// tunnel). None → normal behavior.
    pub endpoint_override: Option<(String, u16)>,
}

pub async fn connect(profile: &Profile, opts: ConnectOptions) -> Result<Connected, AppError> {
    // A secondary (isolated) connection reuses an existing tunnel's local
    // endpoint instead of opening its own ssh child.
    let isolated = opts.endpoint_override.is_some();
    let (host, port, tunnel) = if let Some((host, port)) = opts.endpoint_override {
        (host, port, None)
    } else {
        let tunnel = match &profile.ssh {
            Some(ssh) if !ssh.host.trim().is_empty() => {
                let passphrase = opts
                    .ssh_passphrase_override
                    .or_else(|| store::get_ssh_passphrase(profile));
                let tunnel = tunnel::open_tunnel(
                    ssh,
                    &profile.host,
                    profile.port,
                    passphrase.as_deref(),
                    opts.ssh_mux_ttl,
                )
                .await
                .inspect_err(|e| {
                    logging::log(
                        "connect",
                        &format!("\"{}\": ssh tunnel failed: {e}", profile.name),
                    );
                })?;
                Some(tunnel)
            }
            _ => None,
        };
        let (host, port) = match &tunnel {
            Some(t) => ("127.0.0.1".to_string(), t.local_port),
            None => (profile.host.clone(), profile.port),
        };
        (host, port, tunnel)
    };

    let mut cfg = tokio_postgres::Config::new();
    cfg.host(&host)
        .port(port)
        .user(&profile.user)
        .dbname(&profile.database)
        .application_name("sql-kai")
        .connect_timeout(Duration::from_secs(10));
    let password = opts.password_override.or_else(|| store::get_password(profile));
    if let Some(pw) = password.filter(|p| !p.is_empty()) {
        cfg.password(&pw);
    }

    let (client, connection) = cfg.connect(NoTls).await.inspect_err(|e| {
        logging::log(
            "connect",
            &format!("\"{}\": pg connect to {host}:{port} failed: {e}", profile.name),
        );
    })?;
    // The connection future resolves when the wire dies — its resolution is
    // the ground truth for "why did this session drop".
    let log_name = profile.name.clone();
    let conn_task = tokio::spawn(async move {
        match connection.await {
            Ok(()) => logging::log(
                "session",
                &format!("\"{log_name}\": connection closed by server or tunnel"),
            ),
            Err(e) => logging::log(
                "session",
                &format!("\"{log_name}\": connection terminated: {e}"),
            ),
        }
    });
    let client = Arc::new(client);
    let cancel = client.cancel_token();

    let server_version = match client.simple_query("SHOW server_version").await {
        Ok(msgs) => msgs
            .iter()
            .find_map(|m| match m {
                SimpleQueryMessage::Row(r) => r.get(0).map(|s| s.to_string()),
                _ => None,
            })
            .unwrap_or_default(),
        Err(_) => String::new(),
    };

    let tunnel_port = tunnel.as_ref().map(|t| t.local_port);
    logging::log(
        "connect",
        &format!(
            "\"{}\": connected to {}:{}{} (PostgreSQL {server_version})",
            profile.name,
            profile.host,
            profile.port,
            tunnel_port
                .map(|p| format!(" via ssh tunnel 127.0.0.1:{p}"))
                .unwrap_or_default(),
        ),
    );
    Ok(Connected {
        session: Session {
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            server_version: server_version.clone(),
            tunnel_port,
            client,
            cancel,
            tx: Arc::new(AtomicU8::new(TxStatus::Idle as u8)),
            isolated,
            _tunnel: tunnel,
            conn_task,
        },
        server_version,
        tunnel_port,
    })
}
