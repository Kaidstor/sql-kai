use std::fs;

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::fsio::{config_path, write_atomic};
use crate::vault;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConfig {
    pub host: String,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub key_path: Option<String>,
    /// Seconds between keepalive pings when idle (ssh ServerAliveInterval).
    /// None = app default, Some(0) = keepalive off.
    #[serde(default)]
    pub keepalive_interval: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    #[serde(default)]
    pub ssh: Option<SshConfig>,
    /// Logical group (e.g. "ms-search"): variations of one database
    /// (prod/test) that share the same saved-query collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Accent color name for telling connections apart (e.g. "rose").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Production database: the UI asks before running data-modifying SQL.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub production: bool,
    /// Whether a DB password is stored in the vault for this profile. Cached
    /// here for offline display; the vault is the source of truth.
    #[serde(default)]
    pub has_password: bool,
    /// Whether an SSH key passphrase is stored in the vault for this profile.
    #[serde(default)]
    pub has_ssh_passphrase: bool,
}

impl Profile {
    /// Copy for the frontend, with the `has_*` flags refreshed from the vault.
    pub fn for_frontend(&self) -> Profile {
        let mut p = self.clone();
        p.has_password = vault::has_secret(&p.id);
        p.has_ssh_passphrase = vault::has_secret(&ssh_secret_key(&p.id));
        p
    }
}

/// Vault key for a profile's SSH key passphrase (the DB password lives under
/// the bare profile id).
fn ssh_secret_key(profile_id: &str) -> String {
    format!("{profile_id}#ssh")
}

fn load_list<T: serde::de::DeserializeOwned>(file: &str) -> Result<Vec<T>, AppError> {
    let path = config_path(file)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(vec![]);
    }
    serde_json::from_str(&raw).map_err(|e| AppError::Msg(format!("{file} is corrupted: {e}")))
}

fn save_list<T: Serialize>(file: &str, items: &[T]) -> Result<(), AppError> {
    let path = config_path(file)?;
    write_atomic(&path, serde_json::to_string_pretty(items).unwrap().as_bytes())?;
    Ok(())
}

pub fn load_profiles() -> Result<Vec<Profile>, AppError> {
    load_list("profiles.json")
}

pub fn save_profiles(profiles: &[Profile]) -> Result<(), AppError> {
    save_list("profiles.json", profiles)
}

pub fn find_profile(id: &str) -> Result<Profile, AppError> {
    load_profiles()?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| AppError::Msg("profile not found".into()))
}

/// `secret`: None = keep existing, Some("") = clear, Some(v) = replace.
/// Writes through to the vault and returns whether a secret is now stored.
fn apply_secret(key: &str, secret: Option<String>, existing_has: bool) -> Result<bool, AppError> {
    match secret {
        Some(v) if v.is_empty() => {
            vault::remove_secret(key)?;
            Ok(false)
        }
        Some(v) => {
            vault::set_secret(key, &v)?;
            Ok(true)
        }
        None => Ok(existing_has),
    }
}

/// `password` / `ssh_passphrase`: None = keep existing, Some("") = clear, Some(v) = replace.
pub fn upsert_profile(
    mut profile: Profile,
    password: Option<String>,
    ssh_passphrase: Option<String>,
) -> Result<Profile, AppError> {
    if profile.id.is_empty() {
        profile.id = uuid::Uuid::new_v4().to_string();
    }
    // ssh treats a bare argv entry starting with '-' as an option
    // (e.g. `-oProxyCommand=…` → command execution), so refuse such host/user
    // before they ever reach `Command::arg`.
    if let Some(ssh) = &profile.ssh {
        if ssh.host.trim_start().starts_with('-') {
            return Err(AppError::Msg("ssh host не может начинаться с '-'".into()));
        }
        if ssh
            .user
            .as_deref()
            .is_some_and(|u| u.trim_start().starts_with('-'))
        {
            return Err(AppError::Msg("ssh user не может начинаться с '-'".into()));
        }
    }
    let existing = find_profile(&profile.id).ok();
    profile.has_password = apply_secret(
        &profile.id,
        password,
        existing.as_ref().map(|e| e.has_password).unwrap_or(false),
    )?;
    profile.has_ssh_passphrase = apply_secret(
        &ssh_secret_key(&profile.id),
        ssh_passphrase,
        existing.map(|e| e.has_ssh_passphrase).unwrap_or(false),
    )?;
    let mut all = load_profiles()?;
    match all.iter_mut().find(|p| p.id == profile.id) {
        Some(slot) => *slot = profile.clone(),
        None => all.push(profile.clone()),
    }
    save_profiles(&all)?;
    Ok(profile.for_frontend())
}

/// Clones a profile as a new variation. Fork semantics: if the original has
/// no group yet, both get group = original name, so the pair shares saved
/// queries; the copy also inherits the stored password.
pub fn duplicate_profile(id: &str) -> Result<Profile, AppError> {
    let mut all = load_profiles()?;
    let original = all
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| AppError::Msg("profile not found".into()))?;
    let group = original
        .group
        .clone()
        .filter(|g| !g.trim().is_empty())
        .unwrap_or_else(|| original.name.clone());
    original.group = Some(group.clone());

    let mut copy = original.clone();
    copy.id = uuid::Uuid::new_v4().to_string();
    copy.name = format!("{} (copy)", original.name);
    copy.group = Some(group);

    // carry the secrets over to the copy's own vault keys
    let original = original.clone();
    if let Some(pw) = get_password(&original) {
        copy.has_password = apply_secret(&copy.id, Some(pw), false)?;
    }
    if let Some(pp) = get_ssh_passphrase(&original) {
        copy.has_ssh_passphrase = apply_secret(&ssh_secret_key(&copy.id), Some(pp), false)?;
    }

    all.push(copy.clone());
    save_profiles(&all)?;
    Ok(copy.for_frontend())
}

pub fn delete_profile(id: &str) -> Result<(), AppError> {
    let mut all = load_profiles()?;
    all.retain(|p| p.id != id);
    save_profiles(&all)?;
    let _ = vault::remove_secret(id);
    let _ = vault::remove_secret(&ssh_secret_key(id));
    Ok(())
}

// --- Saved queries ----------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedQuery {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub sql: String,
    /// None = global collection; Some(key) = shared by every profile whose
    /// group (or, for ungrouped profiles, id) equals `key`.
    #[serde(default)]
    pub scope: Option<String>,
}

pub fn load_queries() -> Result<Vec<SavedQuery>, AppError> {
    load_list("queries.json")
}

pub fn save_queries(queries: &[SavedQuery]) -> Result<(), AppError> {
    save_list("queries.json", queries)
}

/// Empty id = create; same name+scope overwrites that query's SQL.
pub fn upsert_query(mut query: SavedQuery) -> Result<SavedQuery, AppError> {
    let mut all = load_queries()?;
    if query.id.is_empty() {
        if let Some(existing) = all
            .iter_mut()
            .find(|q| q.name == query.name && q.scope == query.scope)
        {
            existing.sql = query.sql.clone();
            let out = existing.clone();
            save_queries(&all)?;
            return Ok(out);
        }
        query.id = uuid::Uuid::new_v4().to_string();
        all.push(query.clone());
    } else {
        match all.iter_mut().find(|q| q.id == query.id) {
            Some(slot) => *slot = query.clone(),
            None => all.push(query.clone()),
        }
    }
    save_queries(&all)?;
    Ok(query)
}

pub fn delete_query(id: &str) -> Result<(), AppError> {
    let mut all = load_queries()?;
    all.retain(|q| q.id != id);
    save_queries(&all)
}

// --- App settings -------------------------------------------------------------
// A single JSON object (settings.json) the frontend owns the schema of — the
// backend only round-trips it, so adding a setting never needs a Rust change.
// The file sits next to profiles.json and is meant to be copied between
// machines as-is.

pub fn settings_path() -> Result<std::path::PathBuf, AppError> {
    config_path("settings.json")
}

pub fn load_settings() -> Result<serde_json::Value, AppError> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let raw = fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&raw)
        .map_err(|e| AppError::Msg(format!("settings.json is corrupted: {e}")))
}

pub fn save_settings(settings: &serde_json::Value) -> Result<(), AppError> {
    if !settings.is_object() {
        return Err(AppError::Msg("settings must be a JSON object".into()));
    }
    let path = settings_path()?;
    write_atomic(&path, serde_json::to_string_pretty(settings).unwrap().as_bytes())?;
    Ok(())
}

// --- Query history ------------------------------------------------------------

/// Newest-first list of queries the user ran from a query tab.
pub const HISTORY_CAP: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub profile_id: String,
    pub profile_name: String,
    pub sql: String,
    /// Epoch milliseconds.
    pub at: i64,
    pub ok: bool,
}

pub fn load_history() -> Result<Vec<HistoryEntry>, AppError> {
    load_list("history.json")
}

pub fn save_history(entries: &[HistoryEntry]) -> Result<(), AppError> {
    save_list("history.json", entries)
}

/// Prepends `entry`; a rerun of the same SQL on the same profile just bumps
/// it to the top. Returns the updated list. The SQL is redacted first so a
/// literal password never lands on disk (see [`redact_secrets`]).
pub fn record_history(mut entry: HistoryEntry) -> Result<Vec<HistoryEntry>, AppError> {
    entry.sql = redact_secrets(&entry.sql);
    let mut all = load_history()?;
    all.retain(|h| !(h.profile_id == entry.profile_id && h.sql == entry.sql));
    all.insert(0, entry);
    all.truncate(HISTORY_CAP);
    save_history(&all)?;
    Ok(all)
}

/// Masks single-quoted literals that follow a credential keyword so history
/// never stores a plaintext password: `... PASSWORD 'hunter2'` → `... PASSWORD
/// '***'`. Covers `PASSWORD` and `IDENTIFIED BY` with their single-quoted
/// argument; everything else passes through unchanged.
///
/// Keyword matching is ASCII case-insensitive over the original bytes — we
/// never index a `to_lowercase()` copy, whose byte length can differ from the
/// source (e.g. `İ` → `i̇`) and would slice at a non-char boundary and panic.
pub fn redact_secrets(sql: &str) -> String {
    const KEYWORDS: [&str; 2] = ["password", "identified by"];
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < bytes.len() {
        // At a keyword boundary? (start-of-string or non-alnum before it)
        let boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let kw = if boundary {
            KEYWORDS.iter().copied().find(|&k| {
                bytes.len() - i >= k.len()
                    && bytes[i..i + k.len()].eq_ignore_ascii_case(k.as_bytes())
            })
        } else {
            None
        };
        let Some(kw) = kw else {
            // `i` always lands on a char boundary of the original string.
            let ch = sql[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        };
        out.push_str(&sql[i..i + kw.len()]);
        i += kw.len();
        // copy whitespace up to the quoted literal
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            out.push(bytes[i] as char);
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'\'' {
            // single-quoted string, '' is an escaped quote
            out.push_str("'***'");
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
        }
    }
    out
}

pub fn delete_history_entry(id: &str) -> Result<Vec<HistoryEntry>, AppError> {
    let mut all = load_history()?;
    all.retain(|h| h.id != id);
    save_history(&all)?;
    Ok(all)
}

/// One-time import of the pre-disk localStorage history: entries already on
/// disk win, imported ones fill in behind by recency.
pub fn import_history(entries: Vec<HistoryEntry>) -> Result<Vec<HistoryEntry>, AppError> {
    let mut all = load_history()?;
    for mut e in entries {
        e.sql = redact_secrets(&e.sql);
        if !all
            .iter()
            .any(|h| h.profile_id == e.profile_id && h.sql == e.sql)
        {
            all.push(e);
        }
    }
    all.sort_by_key(|h| std::cmp::Reverse(h.at));
    all.truncate(HISTORY_CAP);
    save_history(&all)?;
    Ok(all)
}

pub fn get_password(profile: &Profile) -> Option<String> {
    vault::get_secret(&profile.id)
}

pub fn get_ssh_passphrase(profile: &Profile) -> Option<String> {
    vault::get_secret(&ssh_secret_key(&profile.id))
}

#[cfg(test)]
mod tests {
    use super::redact_secrets;

    #[test]
    fn redacts_password_literals() {
        assert_eq!(
            redact_secrets("ALTER ROLE app PASSWORD 'hunter2'"),
            "ALTER ROLE app PASSWORD '***'"
        );
        // case-insensitive keyword, extra whitespace
        assert_eq!(
            redact_secrets("create role x password   'p@ss'"),
            "create role x password   '***'"
        );
        // IDENTIFIED BY
        assert_eq!(
            redact_secrets("... IDENTIFIED BY 'sekret'"),
            "... IDENTIFIED BY '***'"
        );
        // embedded escaped quote inside the literal is consumed whole
        assert_eq!(
            redact_secrets("PASSWORD 'a''b' NOT NULL"),
            "PASSWORD '***' NOT NULL"
        );
    }

    #[test]
    fn leaves_ordinary_sql_untouched() {
        let sql = "SELECT id, name FROM users WHERE status = 'active' ORDER BY id";
        assert_eq!(redact_secrets(sql), sql);
        // a column literally named password_hash must not trigger on the column
        let sql2 = "SELECT password_hash FROM users";
        assert_eq!(redact_secrets(sql2), sql2);
    }

    #[test]
    fn does_not_panic_on_length_changing_unicode() {
        // `İ` (U+0130) lowercases to 3 bytes — indexing a to_lowercase() copy by
        // the original byte offset used to slice at a non-char boundary → panic.
        assert_eq!(redact_secrets("aİb"), "aİb");
        assert_eq!(
            redact_secrets("SELECT 'İ' PASSWORD 'x'"),
            "SELECT 'İ' PASSWORD '***'"
        );
        // KELVIN SIGN also lowercases to a shorter/different byte form
        let k = "\u{212A}password 'p'";
        assert_eq!(redact_secrets(k), "\u{212A}password '***'");
    }
}
