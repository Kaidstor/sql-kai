//! Filesystem helpers shared by the config store and the vault.

use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::error::AppError;

const LOCK_FILE: &str = ".config.lock";

/// In-process gate: the outermost [`lock_config`] guard on a thread holds this
/// for its whole critical section, and the OS `flock` rides along with it — so
/// within one process only one thread at a time even attempts the flock.
static PROC_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// Reentrancy depth for the current thread. `upsert_profile` takes the lock
    /// and then calls `vault::set_secret`, which takes it again — a plain flock
    /// self-deadlocks on a second exclusive acquire, so nested acquisitions on
    /// the same thread are ref-counted no-ops.
    static LOCK_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// RAII guard for the cross-process config lock (see [`lock_config`]). The
/// outermost guard on a thread owns the process mutex + the flock'd file;
/// nested guards own nothing and only decrement the reentrancy depth on drop.
pub struct ConfigLock {
    // File first so its Drop (releasing the flock) runs before the mutex frees.
    _held: Option<(File, MutexGuard<'static, ()>)>,
}

/// Acquires an exclusive, cross-process advisory lock over all config writes
/// (the vault and the JSON store), so a GUI and a CLI process can't lost-update
/// each other's files. Reentrant within a thread; blocks until granted. Hold
/// the returned guard for the whole read-modify-write sequence.
///
/// Lock order: this is always the OUTERMOST lock — acquire it before the vault
/// mutex (and before any store file op), never the other way round.
pub fn lock_config() -> Result<ConfigLock, AppError> {
    if LOCK_DEPTH.with(Cell::get) > 0 {
        LOCK_DEPTH.with(|d| d.set(d.get() + 1));
        return Ok(ConfigLock { _held: None });
    }
    // A poisoned PROC_LOCK guards only () — nothing to corrupt, so recover.
    let guard = PROC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = config_path(LOCK_FILE)?;
    // Only the flock matters — never truncate/read the lock file's contents.
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    os_lock_exclusive(&file)?;
    LOCK_DEPTH.with(|d| d.set(1));
    Ok(ConfigLock {
        _held: Some((file, guard)),
    })
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        LOCK_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        // For the outermost guard `_held` drops here: closing the file releases
        // the flock, then the process mutex frees. Nested guards hold nothing.
    }
}

#[cfg(unix)]
fn os_lock_exclusive(file: &File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // LOCK_EX is released automatically when the fd closes (guard drop).
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn os_lock_exclusive(_file: &File) -> std::io::Result<()> {
    Ok(()) // best-effort: no cross-process lock available
}

/// Path of `file` inside the app's config dir (created on demand).
/// `SQL_KAI_CONFIG_DIR` overrides the location (isolated runs, tests).
pub fn config_path(file: &str) -> Result<PathBuf, AppError> {
    let dir = match std::env::var_os("SQL_KAI_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(custom) => PathBuf::from(custom),
        None => {
            let base = dirs::config_dir()
                .ok_or_else(|| AppError::Msg("cannot resolve user config dir".into()))?;
            let new = base.join("sql-kai");
            let old = base.join("sql-tauri");
            // v1.0: каталог переехал под бренд sql-kai — старый переносим
            // целиком (одноразовый атомарный rename); если rename не удался,
            // продолжаем работать со старым, чтобы не потерять данные.
            if !new.exists() && old.exists() {
                let _ = fs::rename(&old, &new);
            }
            if old.exists() && !new.exists() {
                old
            } else {
                new
            }
        }
    };
    fs::create_dir_all(&dir)?;
    Ok(dir.join(file))
}

/// Crash-safe replace: write a 0600 sibling temp file, fsync, then rename over
/// the target — a crash mid-write can never truncate the existing file.
///
/// A symlinked target is resolved first: renaming over the link would replace
/// it with a plain file, so an `~/.cursor/mcp.json` stowed from a dotfiles repo
/// would lose its link and the repo would keep the stale content. The temp file
/// is then a sibling of the *real* file, which also keeps the rename on one
/// filesystem.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    // canonicalize fails when the target does not exist yet — then there is no
    // link to follow and the given path is already the real one.
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = resolved.as_path();
    let tmp = path.with_extension("tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A symlinked target must survive the write: otherwise a stowed
    /// `~/.cursor/mcp.json` turns into a plain file and the edit is lost on the
    /// next restow, while the repo copy stays stale.
    #[test]
    fn write_atomic_writes_through_a_symlink() {
        let dir = std::env::temp_dir().join(format!("sql-kai-fsio-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.json");
        fs::write(&real, b"old").unwrap();
        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_atomic(&link, b"new").unwrap();
        assert!(
            fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
            "симлинк заменён обычным файлом"
        );
        assert_eq!(fs::read(&real).unwrap(), b"new");

        // обычный файл и новый файл пишутся как раньше
        write_atomic(&real, b"plain").unwrap();
        assert_eq!(fs::read(&real).unwrap(), b"plain");
        let fresh = dir.join("fresh.json");
        write_atomic(&fresh, b"hi").unwrap();
        assert_eq!(fs::read(&fresh).unwrap(), b"hi");
        let _ = fs::remove_dir_all(&dir);
    }
}
