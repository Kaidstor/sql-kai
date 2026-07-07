use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::AppError;
use crate::logging;
use crate::store::SshConfig;

const ASKPASS_ENV: &str = "SQL_TAURI_SSH_PASSPHRASE";

/// Writes a helper that echoes the passphrase env var back to ssh.
/// ssh refuses to read a passphrase from stdin without a TTY, but it will
/// run SSH_ASKPASS — this keeps the secret out of argv (env only).
fn ensure_askpass_script() -> Result<PathBuf, AppError> {
    let path = std::env::temp_dir().join("sql-tauri-askpass.sh");
    fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' \"${ASKPASS_ENV}\"\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

/// A local port forwarded to the database through a supervised `ssh -N -L` child.
/// Killing the child (on Drop) tears the tunnel down.
pub struct Tunnel {
    pub local_port: u16,
    child: Child,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        // Distinguish "we tore it down" from "it was already dead" — the
        // latter means the drop originated on the ssh/network side.
        match self.child.try_wait() {
            Ok(Some(status)) => logging::log(
                "tunnel",
                &format!(
                    "local port {}: ssh had already exited ({status}) before teardown",
                    self.local_port
                ),
            ),
            _ => logging::log(
                "tunnel",
                &format!("local port {}: closed by the app", self.local_port),
            ),
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn free_port() -> Result<u16, AppError> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

// --- ssh target / auth helpers ----------------------------------------------

fn ssh_target(ssh: &SshConfig) -> String {
    match ssh.user.as_deref().filter(|u| !u.trim().is_empty()) {
        Some(user) => format!("{user}@{}", ssh.host),
        None => ssh.host.clone(),
    }
}

/// Keepalive pings so an idle tunnel survives NAT/firewall timeouts.
/// Per-profile ServerAliveInterval, 15s when unset; 0 = ssh's "off".
fn push_keepalive(cmd: &mut Command, ssh: &SshConfig) {
    let interval = ssh.keepalive_interval.unwrap_or(15);
    cmd.args(["-o", &format!("ServerAliveInterval={interval}")])
        .args(["-o", "ServerAliveCountMax=3"]);
}

/// Connection-identifying args (`-p`/`-i` then the target) — identical between
/// the master and its forward client so ssh treats them as the same host.
fn push_target(cmd: &mut Command, ssh: &SshConfig) {
    if let Some(port) = ssh.port {
        cmd.arg("-p").arg(port.to_string());
    }
    if let Some(key) = ssh.key_path.as_deref().filter(|k| !k.trim().is_empty()) {
        cmd.arg("-i").arg(key);
    }
    cmd.arg(ssh_target(ssh));
}

/// Without a passphrase: BatchMode=yes — auth must come from keys / ssh-agent /
/// ~/.ssh/config, since a GUI child process cannot answer interactive prompts.
/// With a passphrase: SSH_ASKPASS feeds it to ssh for decrypting the key;
/// password/keyboard-interactive auth stays off (parity with BatchMode).
fn apply_auth(cmd: &mut Command, passphrase: Option<&str>) -> Result<(), AppError> {
    match passphrase {
        Some(pp) if !pp.is_empty() => {
            cmd.args(["-o", "PasswordAuthentication=no"])
                .args(["-o", "KbdInteractiveAuthentication=no"])
                .args(["-o", "NumberOfPasswordPrompts=1"])
                .env("SSH_ASKPASS", ensure_askpass_script()?)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env(ASKPASS_ENV, pp);
            // Pre-8.4 ssh has no SSH_ASKPASS_REQUIRE and only runs the
            // askpass helper when DISPLAY is set.
            if std::env::var_os("DISPLAY").is_none() {
                cmd.env("DISPLAY", ":0");
            }
        }
        _ => {
            cmd.args(["-o", "BatchMode=yes"]);
        }
    }
    Ok(())
}

// --- ControlMaster multiplexing (CLI: reuse ssh auth across kai runs) --------
//
// The slow part of a tunnel is the ssh handshake + auth, not the forward. With
// a persistent ControlMaster the first run pays for auth and leaves a
// background master alive (ControlPersist); later runs attach their `-L`
// forward to it in ~1 RTT — no re-auth. The forward client is still held by the
// process and dies on Drop; only the master persists.

/// Short dir for control sockets. Unix socket paths cap at ~104 bytes, so this
/// deliberately avoids the long Application Support path. Per-user under /tmp, 0700.
fn mux_dir() -> Result<PathBuf, AppError> {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let dir = Path::new("/tmp").join(format!("kai-ssh-mux-{user}"));
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// Socket name fully identifies the target so every run to the same host+port+user
/// shares one master (`:` and `@` are legal in unix filenames).
fn control_name(ssh: &SshConfig) -> String {
    let port = ssh.port.unwrap_or(22);
    match ssh.user.as_deref().filter(|u| !u.trim().is_empty()) {
        Some(user) => format!("{user}@{}:{port}", ssh.host),
        None => format!("{}:{port}", ssh.host),
    }
}

fn control_path(ssh: &SshConfig) -> Result<PathBuf, AppError> {
    Ok(mux_dir()?.join(control_name(ssh)))
}

/// Host portion of a `[user@]host:port` socket name (for `ssh -O` commands).
fn host_of(name: &str) -> String {
    let after_user = name.rsplit_once('@').map(|(_, h)| h).unwrap_or(name);
    after_user
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(after_user)
        .to_string()
}

fn master_alive(ctl: &Path, target: &str) -> bool {
    Command::new("ssh")
        .arg("-O")
        .arg("check")
        .arg("-S")
        .arg(ctl)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort: ensure a background auth master exists so the forward can attach
/// without re-authenticating. Failure is non-fatal — the forward then falls back
/// to a direct (slower) connection.
fn ensure_master(ssh: &SshConfig, passphrase: Option<&str>, ttl: u32, ctl: &Path) {
    let target = ssh_target(ssh);
    if master_alive(ctl, &target) {
        return;
    }
    let mut cmd = Command::new("ssh");
    cmd.arg("-M")
        .arg("-S")
        .arg(ctl)
        .arg("-f") // fork to background after auth
        .arg("-N")
        .args(["-o", &format!("ControlPersist={ttl}")])
        .args(["-o", "ConnectTimeout=10"]);
    push_keepalive(&mut cmd, ssh);
    if apply_auth(&mut cmd, passphrase).is_err() {
        return;
    }
    push_target(&mut cmd, ssh);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // `-f` returns once the master is up; ignore the result (forward will report
    // any real auth failure).
    let _ = cmd.status();
}

/// One live ssh master, for `kai tunnel list`.
pub struct MasterInfo {
    pub target: String,
    pub alive: bool,
}

pub fn list_masters() -> Vec<MasterInfo> {
    let dir = match mux_dir() {
        Ok(d) => d,
        Err(_) => return vec![],
    };
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let alive = master_alive(&e.path(), &host_of(&name));
            out.push(MasterInfo { target: name, alive });
        }
    }
    out
}

/// Closes masters (`ssh -O exit` then removes the socket). `only` matches a
/// socket name or its host part; None closes all. Returns how many were closed.
pub fn close_masters(only: Option<&str>) -> usize {
    let dir = match mux_dir() {
        Ok(d) => d,
        Err(_) => return 0,
    };
    let mut n = 0;
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(f) = only {
                if name != f && host_of(&name) != f {
                    continue;
                }
            }
            let _ = Command::new("ssh")
                .arg("-O")
                .arg("exit")
                .arg("-S")
                .arg(e.path())
                .arg(host_of(&name))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = fs::remove_file(e.path());
            n += 1;
        }
    }
    n
}

/// Spawns `ssh -N -L 127.0.0.1:<local>:<db_host>:<db_port> <target>` and waits until
/// the forwarded port accepts connections (ssh binds it only after auth succeeds).
///
/// `mux_ttl`: Some(ttl) reuses a persistent ControlMaster (CLI — the master
/// lingers `ttl` seconds idle so later runs skip re-auth); None opens a plain
/// standalone tunnel (GUI, which already holds one for the whole session).
pub async fn open_tunnel(
    ssh: &SshConfig,
    db_host: &str,
    db_port: u16,
    passphrase: Option<&str>,
    mux_ttl: Option<u32>,
) -> Result<Tunnel, AppError> {
    let local_port = free_port()?;
    let target = ssh_target(ssh);
    logging::log(
        "tunnel",
        &format!(
            "opening {target}: -L 127.0.0.1:{local_port}:{db_host}:{db_port}, keepalive {}s",
            ssh.keepalive_interval.unwrap_or(15)
        ),
    );

    let ctl = match mux_ttl {
        Some(ttl) => match control_path(ssh) {
            Ok(p) => {
                ensure_master(ssh, passphrase, ttl, &p);
                Some(p)
            }
            Err(_) => None,
        },
        None => None,
    };

    let mut cmd = Command::new("ssh");
    cmd.arg("-N");
    apply_auth(&mut cmd, passphrase)?;
    cmd.args(["-o", "ExitOnForwardFailure=yes"])
        .args(["-o", "ConnectTimeout=10"]);
    push_keepalive(&mut cmd, ssh);
    if let Some(ctl) = &ctl {
        // Attach to the shared master (fast). If it's gone, ssh falls back to a
        // direct connection instead of creating a new master.
        cmd.args(["-o", "ControlMaster=no"]).arg("-S").arg(ctl);
    }
    cmd.arg("-L")
        .arg(format!("127.0.0.1:{local_port}:{db_host}:{db_port}"));
    push_target(&mut cmd, ssh);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Msg(format!("failed to spawn ssh: {e}")))?;

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            let mut err = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = stderr.read_to_string(&mut err);
            }
            return Err(AppError::Msg(format!(
                "ssh tunnel exited ({status}): {}",
                err.trim()
            )));
        }
        match tokio::net::TcpStream::connect(("127.0.0.1", local_port)).await {
            Ok(_) => break,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::Msg(
                    "ssh tunnel: timed out waiting for the forwarded port".into(),
                ));
            }
        }
    }

    // Stream ssh's stderr into the log for the tunnel's lifetime: when the
    // forward dies later (keepalive timeout, network drop, server reboot),
    // ssh's last words ("Timeout, server not responding", "broken pipe", …)
    // are the diagnosis. The thread ends at EOF — i.e. when ssh exits.
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    logging::log("ssh", &format!("{target}: {line}"));
                }
            }
            logging::log("ssh", &format!("{target}: process exited"));
        });
    }

    Ok(Tunnel { local_port, child })
}
