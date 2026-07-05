use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::AppError;
use crate::store::SshConfig;

const ASKPASS_ENV: &str = "SQL_TAURI_SSH_PASSPHRASE";

/// Writes a helper that echoes the passphrase env var back to ssh.
/// ssh refuses to read a passphrase from stdin without a TTY, but it will
/// run SSH_ASKPASS — this keeps the secret out of argv (env only).
fn ensure_askpass_script() -> Result<PathBuf, AppError> {
    let path = std::env::temp_dir().join("sql-tauri-askpass.sh");
    std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' \"${ASKPASS_ENV}\"\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn free_port() -> Result<u16, AppError> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

/// Spawns `ssh -N -L 127.0.0.1:<local>:<db_host>:<db_port> <target>` and waits until
/// the forwarded port accepts connections (ssh binds it only after auth succeeds).
///
/// Without a passphrase: BatchMode=yes — auth must come from keys / ssh-agent /
/// ~/.ssh/config, since a GUI child process cannot answer interactive prompts.
/// With a passphrase: SSH_ASKPASS feeds it to ssh for decrypting the key;
/// password/keyboard-interactive auth stays off (parity with BatchMode).
pub async fn open_tunnel(
    ssh: &SshConfig,
    db_host: &str,
    db_port: u16,
    passphrase: Option<&str>,
) -> Result<Tunnel, AppError> {
    let local_port = free_port()?;

    let mut cmd = Command::new("ssh");
    cmd.arg("-N");
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
    cmd.args(["-o", "ExitOnForwardFailure=yes"])
        .args(["-o", "ConnectTimeout=10"])
        .args(["-o", "ServerAliveInterval=15"])
        .args(["-o", "ServerAliveCountMax=3"])
        .arg("-L")
        .arg(format!("127.0.0.1:{local_port}:{db_host}:{db_port}"));
    if let Some(port) = ssh.port {
        cmd.arg("-p").arg(port.to_string());
    }
    if let Some(key) = ssh.key_path.as_deref().filter(|k| !k.trim().is_empty()) {
        cmd.arg("-i").arg(key);
    }
    let target = match ssh.user.as_deref().filter(|u| !u.trim().is_empty()) {
        Some(user) => format!("{user}@{}", ssh.host),
        None => ssh.host.clone(),
    };
    cmd.arg(target);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());

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

    Ok(Tunnel { local_port, child })
}
