//! Append-only diagnostics log (sql-kai.log next to profiles.json).
//!
//! Purpose-built for "why did my connection drop?": connection lifecycle,
//! ssh stderr and the pg termination reason land here with timestamps.
//! Writes are best-effort — logging must never break a query path.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, Once};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::AppError;
use crate::fsio::config_path;

const LOG_FILE: &str = "sql-kai.log";
/// Rotate (move to .old) past this size so the log never grows unbounded.
const MAX_LEN: u64 = 1024 * 1024;

/// Serializes writers (ssh stderr threads, async tasks, commands).
static LOCK: Mutex<()> = Mutex::new(());
/// One-shot migration of a pre-existing world/group-readable log to 0600.
static TIGHTEN: Once = Once::new();

pub fn log_path() -> Result<PathBuf, AppError> {
    config_path(LOG_FILE)
}

/// Appends `[timestamp] [scope] message`; never panics, errors are swallowed.
pub fn log(scope: &str, message: &str) {
    let Ok(path) = log_path() else { return };
    let _guard = LOCK.lock();
    // The diagnostics log can hold ssh stderr / connection strings — keep it
    // owner-only. New files are created 0600 below; tighten an existing broader
    // one once per process.
    #[cfg(unix)]
    TIGHTEN.call_once(|| {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.permissions().mode() & 0o077 != 0 {
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    });
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > MAX_LEN {
            let _ = std::fs::rename(&path, path.with_extension("log.old"));
        }
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!(
        "[{}] [{scope}] {}\n",
        format_utc(secs),
        message.trim_end()
    );
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    if let Ok(mut f) = opts.open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Tail of the log for the in-app viewer: the last `max` bytes, cut at a
/// line boundary. The file rotates at [`MAX_LEN`], so this stays cheap.
pub fn read_tail(max: usize) -> Result<String, AppError> {
    let path = log_path()?;
    if !path.exists() {
        return Ok(String::new());
    }
    let raw = fs::read(&path)?;
    let slice = if raw.len() > max {
        let start = raw.len() - max;
        // skip the (probably torn) first line of the window
        let from = raw[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| start + i + 1)
            .unwrap_or(start);
        &raw[from..]
    } else {
        &raw[..]
    };
    Ok(String::from_utf8_lossy(slice).into_owned())
}

/// Epoch seconds → UTC wall clock, without a chrono dependency
/// (civil-from-days, H. Hinnant).
fn format_utc(secs: u64) -> String {
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{d:02} {h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::format_utc;

    #[test]
    fn formats_known_epochs() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00Z");
        // leap day
        assert_eq!(format_utc(1_709_208_000), "2024-02-29 12:00:00Z");
        // end of year
        assert_eq!(format_utc(1_735_689_599), "2024-12-31 23:59:59Z");
    }
}
