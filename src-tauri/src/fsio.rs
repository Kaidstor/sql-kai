//! Filesystem helpers shared by the config store and the vault.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::AppError;

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
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
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
