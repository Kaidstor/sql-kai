use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::Arc;
use std::time::Duration;

use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

use super::sqltext::TxStatus;
use crate::error::AppError;
use crate::logging;
use crate::store::{self, Profile, SslConfig};
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
    /// Whether the currently-open transaction (meaningful only when [`tx`] is
    /// not `Idle`) was opened under write mode, i.e. is read-write. The broker
    /// uses this to refuse read-only calls that would otherwise continue a
    /// read-write transaction a prior `--write` call left open —
    /// `default_transaction_read_only` alone can't cover that, as it only
    /// affects *new* transactions. Always false in the GUI (never read there).
    pub tx_write: Arc<AtomicBool>,
    /// A per-tab secondary connection (own backend pid / transaction) that
    /// reuses the profile's tunnel — not the profile's primary session.
    pub isolated: bool,
    /// Resolves with the close reason the moment the wire dies (the connection
    /// future finished) — hosts take it to react immediately instead of
    /// discovering the corpse on the next call. A deliberate teardown aborts
    /// the sending task, so the receiver errors — that is "not lost".
    pub closed_rx: Option<tokio::sync::oneshot::Receiver<String>>,
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
        // Whether ssh is on is [`Profile::ssh_alias`]'s call; the tunnel itself
        // needs the whole config (key, port, user), hence both halves here.
        let tunnel = match (profile.ssh_alias(), profile.ssh.as_ref()) {
            (Some(_), Some(ssh)) => {
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

    let ssl_enabled = profile.ssl.as_ref().is_some_and(|s| s.enabled);
    let mut cfg = tokio_postgres::Config::new();
    cfg.port(port)
        .user(&profile.user)
        .dbname(&profile.database)
        .application_name("sql-kai")
        .connect_timeout(Duration::from_secs(10));
    // Dialing a local forward (ssh tunnel / isolated reuse of one) with TLS on:
    // TCP must go to the forward, but certificate verification must run against
    // the real server name, not 127.0.0.1 — split them libpq-style: `host`
    // carries the name (SNI + verification), `hostaddr` the address to dial.
    let tls_via_forward = ssl_enabled && host != profile.host;
    match host.parse::<std::net::IpAddr>() {
        Ok(addr) if tls_via_forward => {
            cfg.host(&profile.host);
            cfg.hostaddr(addr);
        }
        _ => {
            cfg.host(&host);
        }
    }
    let password = opts.password_override.or_else(|| store::get_password(profile));
    if let Some(pw) = password.filter(|p| !p.is_empty()) {
        cfg.password(&pw);
    }

    // TLS is explicit opt-in (default — plaintext). It applies to direct
    // connections and through an ssh tunnel alike: with a bastion the
    // bastion→DB leg is outside the ssh encryption, and a server may simply
    // demand TLS regardless of the route (rds.force_ssl, hostssl in pg_hba).
    let ssl = profile.ssl.as_ref().filter(|s| s.enabled);

    if let Some(ssl) = ssl {
        // Require: the server not offering TLS is a failure, never a silent
        // fallback to plaintext.
        cfg.ssl_mode(tokio_postgres::config::SslMode::Require);
        let tls = postgres_native_tls::MakeTlsConnector::new(build_tls_connector(ssl)?);
        let conn = cfg
            .connect(tls)
            .await
            .inspect_err(|e| log_connect_fail(profile, &host, port, e))?;
        build_session(profile, conn.0, conn.1, tunnel, isolated).await
    } else {
        let conn = cfg
            .connect(NoTls)
            .await
            .inspect_err(|e| log_connect_fail(profile, &host, port, e))?;
        build_session(profile, conn.0, conn.1, tunnel, isolated).await
    }
}

/// TLS connector from the profile's [`SslConfig`]: custom CA, client
/// certificate + key (mTLS), server-certificate verification on/off. All files
/// are PEM; `~/` in paths expands to the home directory.
fn build_tls_connector(ssl: &SslConfig) -> Result<native_tls::TlsConnector, AppError> {
    let mut b = native_tls::TlsConnector::builder();
    if !ssl.reject_unauthorized {
        // encrypt-only (libpq `require`): any server certificate is accepted
        b.danger_accept_invalid_certs(true);
        b.danger_accept_invalid_hostnames(true);
    }
    if let Some(ca) = ssl_path(&ssl.ca_cert) {
        let pem = read_pem(&ca, "CA certificate")?;
        let cert = native_tls::Certificate::from_pem(&pem)
            .map_err(|e| AppError::Msg(format!("CA certificate {ca}: {e}")))?;
        b.add_root_certificate(cert);
    }
    match (ssl_path(&ssl.client_cert), ssl_path(&ssl.client_key)) {
        (Some(cert), Some(key)) => {
            let cert_pem = read_pem(&cert, "client certificate")?;
            let key_pem = read_pem(&key, "client certificate key")?;
            let id = native_tls::Identity::from_pkcs8(&cert_pem, &key_pem)
                .map_err(|e| AppError::Msg(format!("client certificate: {e}")))?;
            b.identity(id);
        }
        (None, None) => {}
        _ => {
            return Err(AppError::Msg(
                "a client certificate needs both files: certificate and key".into(),
            ))
        }
    }
    b.build()
        .map_err(|e| AppError::Msg(format!("could not set up TLS: {e}")))
}

/// Non-empty trimmed path from an [`SslConfig`] field, `~/` expanded.
fn ssl_path(v: &Option<String>) -> Option<String> {
    let p = v.as_deref().map(str::trim).filter(|s| !s.is_empty())?;
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|h| h.join(rest).to_string_lossy().into_owned()),
        None => Some(p.to_string()),
    }
}

fn read_pem(path: &str, what: &str) -> Result<Vec<u8>, AppError> {
    std::fs::read(path).map_err(|e| AppError::Msg(format!("{what} {path}: {e}")))
}

fn log_connect_fail(profile: &Profile, host: &str, port: u16, e: &tokio_postgres::Error) {
    logging::log(
        "connect",
        &format!("\"{}\": pg connect to {host}:{port} failed: {e}", profile.name),
    );
}

/// Spawns the connection driver task and assembles the live [`Session`].
/// Generic over the connection future so the NoTls and TLS paths share it — the
/// future is the only thing that differs between them.
async fn build_session(
    profile: &Profile,
    client: Client,
    connection: impl std::future::Future<Output = Result<(), tokio_postgres::Error>> + Send + 'static,
    tunnel: Option<Tunnel>,
    isolated: bool,
) -> Result<Connected, AppError> {
    // The connection future resolves when the wire dies — its resolution is
    // the ground truth for "why did this session drop". The oneshot carries
    // that moment to the session's host (see Session::closed_rx).
    let log_name = profile.name.clone();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let conn_task = tokio::spawn(async move {
        let reason = match connection.await {
            Ok(()) => "connection closed by server or tunnel".to_string(),
            Err(e) => format!("connection terminated: {e}"),
        };
        logging::log("session", &format!("\"{log_name}\": {reason}"));
        let _ = closed_tx.send(reason);
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
            tx_write: Arc::new(AtomicBool::new(false)),
            isolated,
            closed_rx: Some(closed_rx),
            _tunnel: tunnel,
            conn_task,
        },
        server_version,
        tunnel_port,
    })
}
