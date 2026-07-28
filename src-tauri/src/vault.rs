//! Secret vault: one random Data Encryption Key (DEK) encrypts every stored
//! secret (DB passwords, SSH passphrases) as a single AES-256-GCM blob on disk.
//! The DEK itself is wrapped by a key derived from the app master password
//! (Argon2id). Unlocking once decrypts the DEK into memory for the session, so
//! there is no per-connection OS-keychain prompt.
//!
//! Touch ID fast path (macOS): the same DEK is optionally stashed in a
//! biometric-gated keychain item (see `biometric.rs`); either unlock route
//! yields the same DEK, the master password always remains the fallback.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::biometric::{self, BioError};
use crate::error::AppError;
use crate::fsio::{config_path, write_atomic};

const VAULT_FILE: &str = "vault.json";
const VERSION: u32 = 1;

/// Env-переменная с мастер-паролем vault. Единый префикс `SQL_KAI_*` — как у
/// `SQL_KAI_SSH_PASSPHRASE` и `SQL_KAI_CONFIG_DIR`.
pub const MASTER_PASSWORD_ENV: &str = "SQL_KAI_VAULT_PASSWORD";

/// Вычищает мастер-пароль vault из окружения дочернего процесса — ssh/sec и
/// прочие внешние бинари не должны его видеть.
pub fn scrub_master_password_env(cmd: &mut std::process::Command) {
    cmd.env_remove(MASTER_PASSWORD_ENV);
}

/// То же для tokio-команд (ACP-агенты): у tokio свой тип Command.
pub fn scrub_master_password_env_tokio(cmd: &mut tokio::process::Command) {
    cmd.env_remove(MASTER_PASSWORD_ENV);
}

// Argon2id cost parameters (stored in the file so re-derivation stays correct
// even if these defaults change later).
const ARGON_M_COST: u32 = 19_456; // KiB (~19 MiB)
const ARGON_T_COST: u32 = 2;
const ARGON_P_COST: u32 = 1;

/// Decrypted vault held for the lifetime of the app session.
struct Unlocked {
    dek: [u8; 32],
    secrets: BTreeMap<String, String>,
    /// On-disk header (kdf, pw_wrap, flags) — kept in memory so persisting
    /// secrets never has to re-read the file.
    file: VaultFile,
}

impl Drop for Unlocked {
    /// Best-effort scrub of key material when the vault locks (or app exits).
    fn drop(&mut self) {
        self.dek.zeroize();
        for (_, mut v) in std::mem::take(&mut self.secrets) {
            v.zeroize();
        }
    }
}

static VAULT: Mutex<Option<Unlocked>> = Mutex::new(None);

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct KdfParams {
    salt: String,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
}

/// AES-GCM ciphertext with its nonce, both base64.
#[derive(Serialize, Deserialize, Clone)]
struct Enc {
    nonce: String,
    ct: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct VaultFile {
    version: u32,
    kdf: KdfParams,
    /// AES-GCM(KEK_pw, DEK)
    pw_wrap: Enc,
    /// AES-GCM(DEK, json(secrets map))
    secrets: Enc,
    /// A copy of the DEK sits in the biometric keychain (Touch ID unlock).
    #[serde(default)]
    biometric: bool,
}

fn vault_path() -> Result<PathBuf, AppError> {
    config_path(VAULT_FILE)
}

fn write_file(file: &VaultFile) -> Result<(), AppError> {
    let path = vault_path()?;
    write_atomic(&path, serde_json::to_string_pretty(file).unwrap().as_bytes())?;
    Ok(())
}

fn read_file() -> Result<VaultFile, AppError> {
    let raw = fs::read_to_string(vault_path()?)?;
    serde_json::from_str(&raw).map_err(|e| AppError::Msg(format!("vault is corrupted: {e}")))
}

fn argon2(params: &KdfParams) -> Result<Argon2<'static>, AppError> {
    let p = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(32))
        .map_err(|e| AppError::Msg(format!("bad kdf params: {e}")))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, p))
}

fn derive_kek(password: &str, params: &KdfParams) -> Result<[u8; 32], AppError> {
    let salt = B64
        .decode(&params.salt)
        .map_err(|e| AppError::Msg(format!("bad kdf salt: {e}")))?;
    let mut key = [0u8; 32];
    argon2(params)?
        .hash_password_into(password.as_bytes(), &salt, &mut key)
        .map_err(|e| AppError::Msg(format!("key derivation failed: {e}")))?;
    Ok(key)
}

fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Enc, AppError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| AppError::Msg("encryption failed".into()))?;
    Ok(Enc {
        nonce: B64.encode(nonce),
        ct: B64.encode(ct),
    })
}

/// Returns the plaintext, or a generic error (used to signal a wrong password).
fn decrypt(key: &[u8; 32], enc: &Enc) -> Result<Vec<u8>, AppError> {
    let nonce = B64
        .decode(&enc.nonce)
        .map_err(|e| AppError::Msg(format!("bad nonce: {e}")))?;
    let ct = B64
        .decode(&enc.ct)
        .map_err(|e| AppError::Msg(format!("bad ciphertext: {e}")))?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(&nonce), ct.as_ref())
        .map_err(|_| AppError::Msg("decryption failed".into()))
}

// --- Public API -------------------------------------------------------------

pub fn exists() -> bool {
    vault_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn is_unlocked() -> bool {
    VAULT.lock().unwrap().is_some()
}

/// First-run: create the vault protected by `password` and unlock it.
pub fn setup(password: &str) -> Result<(), AppError> {
    if password.is_empty() {
        return Err(AppError::Msg("master password must not be empty".into()));
    }
    if exists() {
        return Err(AppError::Msg("vault already exists".into()));
    }
    let mut dek = [0u8; 32];
    OsRng.fill_bytes(&mut dek);
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);

    let kdf = KdfParams {
        salt: B64.encode(salt),
        m_cost: ARGON_M_COST,
        t_cost: ARGON_T_COST,
        p_cost: ARGON_P_COST,
    };
    let kek = derive_kek(password, &kdf)?;
    let secrets: BTreeMap<String, String> = BTreeMap::new();
    let file = VaultFile {
        version: VERSION,
        pw_wrap: encrypt(&kek, &dek)?,
        secrets: encrypt(&dek, &serde_json::to_vec(&secrets).unwrap())?,
        kdf,
        biometric: false,
    };
    write_file(&file)?;
    *VAULT.lock().unwrap() = Some(Unlocked { dek, secrets, file });
    Ok(())
}

/// Decrypts the secrets blob with a recovered DEK and installs the session.
fn decrypt_secrets(
    dek: &[u8; 32],
    file: &VaultFile,
) -> Result<BTreeMap<String, String>, AppError> {
    let plain = decrypt(dek, &file.secrets)?;
    serde_json::from_slice(&plain).map_err(|e| AppError::Msg(format!("vault is corrupted: {e}")))
}

fn install_unlocked(dek: [u8; 32], file: VaultFile) -> Result<(), AppError> {
    let secrets = decrypt_secrets(&dek, &file)?;
    *VAULT.lock().unwrap() = Some(Unlocked { dek, secrets, file });
    Ok(())
}

/// Recover the DEK from the master password and hold it for the session.
pub fn unlock_password(password: &str) -> Result<(), AppError> {
    let file = read_file()?;
    let kek = derive_kek(password, &file.kdf)?;
    let dek_vec = decrypt(&kek, &file.pw_wrap)
        .map_err(|_| AppError::Msg("incorrect master password".into()))?;
    let dek: [u8; 32] = dek_vec
        .try_into()
        .map_err(|_| AppError::Msg("vault is corrupted: bad key length".into()))?;
    install_unlocked(dek, file)
}

// --- Biometric (Touch ID) fast path ------------------------------------------

pub fn biometric_supported() -> bool {
    biometric::supported()
}

pub fn biometric_enrolled() -> bool {
    if let Some(v) = VAULT.lock().unwrap().as_ref() {
        return v.file.biometric;
    }
    read_file().map(|f| f.biometric).unwrap_or(false)
}

/// Puts a copy of the session DEK behind Touch ID. Vault must be unlocked.
pub fn enable_biometric() -> Result<(), AppError> {
    // Copy the DEK into the biometric keychain (needs the unlocked vault).
    {
        let guard = VAULT.lock().unwrap();
        let v = guard
            .as_ref()
            .ok_or_else(|| AppError::Msg("vault is locked".into()))?;
        biometric::store_dek(&v.dek).map_err(bio_err)?;
    }
    // Flip the header flag under the config lock, preserving on-disk secrets.
    mutate_vault(|file, _secrets| file.biometric = true)
}

/// Removes the Touch ID copy of the DEK; works locked or unlocked.
pub fn disable_biometric() -> Result<(), AppError> {
    biometric::delete_dek();
    // Unlocked: clear the flag via mutate_vault (config lock; preserves any
    // secrets a concurrent writer added since we unlocked).
    if is_unlocked() {
        return mutate_vault(|file, _secrets| file.biometric = false);
    }
    // Locked: no DEK to re-encrypt with, so only flip the header bit — still
    // under the config lock so it can't clobber a concurrent secret write.
    let _lock = crate::fsio::lock_config()?;
    if let Ok(mut f) = read_file() {
        f.biometric = false;
        write_file(&f)?;
    }
    Ok(())
}

/// Unlocks via the Touch ID keychain item (macOS shows the prompt during the
/// read — blocking, call off the main thread). A stale key (deleted item,
/// recreated vault) auto-disables the fast path with a recovery hint.
pub fn unlock_biometric() -> Result<(), AppError> {
    let file = read_file()?;
    if !file.biometric {
        return Err(AppError::Msg("Touch ID is not set up for this vault".into()));
    }
    let mut dek_vec = match biometric::read_dek() {
        Ok(v) => v,
        Err(BioError::Stale) => return Err(stale_biometric_err()),
        Err(e) => return Err(bio_err(e)),
    };
    let dek: [u8; 32] = match dek_vec.clone().try_into() {
        Ok(d) => d,
        Err(_) => {
            dek_vec.zeroize();
            return Err(stale_biometric_err());
        }
    };
    dek_vec.zeroize();
    match install_unlocked(dek, file) {
        Ok(()) => Ok(()),
        // GCM auth failed — the keychain DEK belongs to an older vault.
        Err(_) => Err(stale_biometric_err()),
    }
}

fn stale_biometric_err() -> AppError {
    let _ = disable_biometric();
    AppError::Msg(
        "the Touch ID key was stale and has been reset — unlock with the master password \
         and enable Touch ID again"
            .into(),
    )
}

fn bio_err(e: BioError) -> AppError {
    AppError::Msg(match e {
        BioError::Cancelled => "cancelled".into(),
        BioError::Stale => "Touch ID key not found".into(),
        BioError::Other(m) => m,
    })
}

// --- CLI trust (keychain, no biometry gate) -----------------------------------

/// Puts a DEK copy in the login keychain readable by the CLI without a prompt
/// (the keychain per-app ACL still applies). Vault must be unlocked.
pub fn enable_cli_trust() -> Result<(), AppError> {
    let guard = VAULT.lock().unwrap();
    let v = guard
        .as_ref()
        .ok_or_else(|| AppError::Msg("vault is locked".into()))?;
    biometric::store_dek_cli(&v.dek).map_err(bio_err)
}

pub fn disable_cli_trust() {
    biometric::delete_dek_cli();
}

/// Whether a CLI-trust DEK copy is present (probes the keychain).
pub fn cli_trust_enrolled() -> bool {
    biometric::read_dek_cli().is_ok()
}

/// Unlocks with the CLI-trust keychain copy of the DEK. A stale key (vault
/// recreated since enrollment) removes itself with a recovery hint.
pub fn unlock_cli_trust() -> Result<(), AppError> {
    let file = read_file()?;
    let mut dek_vec = match biometric::read_dek_cli() {
        Ok(v) => v,
        Err(BioError::Stale) => {
            return Err(AppError::Msg(
                "CLI trust is not set up — run `sql-kai vault trust` first".into(),
            ))
        }
        Err(e) => return Err(bio_err(e)),
    };
    let dek: [u8; 32] = match dek_vec.clone().try_into() {
        Ok(d) => d,
        Err(_) => {
            dek_vec.zeroize();
            return Err(stale_cli_trust_err());
        }
    };
    dek_vec.zeroize();
    install_unlocked(dek, file).map_err(|_| stale_cli_trust_err())
}

fn stale_cli_trust_err() -> AppError {
    disable_cli_trust();
    AppError::Msg(
        "the CLI trust key was stale and has been reset — run `sql-kai vault trust` again".into(),
    )
}

pub fn lock() {
    *VAULT.lock().unwrap() = None;
}

/// Re-reads `vault.json` with the DEK already in memory, picking up secrets
/// another process wrote after this one unlocked.
///
/// Secrets are decrypted once at unlock and kept in memory (the DEK is zeroized
/// on lock), so a long-lived host — the GUI or the CLI holder — never saw a
/// secret added by a separate `sql-kai` process. `sql-kai fork` hits this every
/// time: it creates a profile, stores its password, and the very next query
/// against that profile goes through the already-running holder, which then
/// connects with no password at all.
///
/// No-op when the vault is locked (nothing to refresh) or absent — including
/// when it gets locked *while* this runs: reading and decrypting the file
/// happens with the mutex released, so the user may have hit Lock in the GUI
/// meanwhile. Re-installing unconditionally would quietly unlock it again,
/// leaving secrets in memory while the UI shows the vault as sealed.
pub fn refresh_secrets() -> Result<(), AppError> {
    let dek = match VAULT.lock().unwrap().as_ref() {
        Some(v) => v.dek,
        None => return Ok(()),
    };
    let file = read_file()?;
    let secrets = decrypt_secrets(&dek, &file)?;
    if let Some(v) = VAULT.lock().unwrap().as_mut() {
        v.secrets = secrets;
        v.file = file;
    }
    Ok(())
}

pub fn get_secret(key: &str) -> Option<String> {
    VAULT
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|v| v.secrets.get(key).cloned())
}

pub fn has_secret(key: &str) -> bool {
    VAULT
        .lock()
        .unwrap()
        .as_ref()
        .map(|v| v.secrets.contains_key(key))
        .unwrap_or(false)
}

/// Applies `edit` to the *freshest on-disk* vault (header + decrypted secrets),
/// re-encrypts and writes — all under the cross-process config lock, so a
/// concurrent GUI/CLI writer can't clobber the file (the whole map was rewritten
/// on every change, so a stale in-memory snapshot would drop the other side's
/// secrets). Refreshes the in-memory session to match.
///
/// Fails closed when the on-disk state can't be read back: writing the
/// in-memory snapshot over a file we failed to read would resurrect deleted
/// secrets, drop another process's writes, or roll the vault back to an old
/// DEK. A DEK that no longer opens the file means another process rotated the
/// master password — the in-memory session locks and the user unlocks again.
///
/// Lock order: config lock first (it may already be held reentrantly by a store
/// mutation calling us), then the vault mutex — never the reverse.
fn mutate_vault(
    edit: impl FnOnce(&mut VaultFile, &mut BTreeMap<String, String>),
) -> Result<(), AppError> {
    let _lock = crate::fsio::lock_config()?;
    let mut guard = VAULT.lock().unwrap();
    let v = guard
        .as_mut()
        .ok_or_else(|| AppError::Msg("vault is locked".into()))?;
    let mut file =
        read_file().map_err(|e| AppError::Msg(format!("vault not updated — {e}")))?;
    let plain = match decrypt(&v.dek, &file.secrets) {
        Ok(plain) => plain,
        Err(_) => {
            *guard = None;
            return Err(AppError::Msg(
                "vault not updated — its key changed on disk (master password \
                 rotated by another process?); the vault is now locked, unlock \
                 it again"
                    .into(),
            ));
        }
    };
    let mut secrets: BTreeMap<String, String> = serde_json::from_slice(&plain)
        .map_err(|e| AppError::Msg(format!("vault not updated — secrets corrupted: {e}")))?;
    edit(&mut file, &mut secrets);
    file.secrets = encrypt(&v.dek, &serde_json::to_vec(&secrets).unwrap())?;
    write_file(&file)?;
    v.secrets = secrets;
    v.file = file;
    Ok(())
}

/// Re-read on-disk secrets into the unlocked session, keeping the current DEK.
/// Lets an already-unlocked GUI pick up passwords a CLI (discover/import) wrote
/// after this process unlocked. No-op if locked; leaves the snapshot intact if
/// the file won't decrypt with our DEK (concurrent master-password rotation).
///
/// Lock order: config lock first, then the vault mutex — same as `mutate_vault`.
pub fn reload_secrets() -> Result<(), AppError> {
    let _lock = crate::fsio::lock_config()?;
    let mut guard = VAULT.lock().unwrap();
    let Some(v) = guard.as_mut() else {
        return Ok(());
    };
    let file = read_file()?;
    let plain = decrypt(&v.dek, &file.secrets)?;
    let secrets: BTreeMap<String, String> =
        serde_json::from_slice(&plain).map_err(|e| AppError::Msg(format!("vault reload: {e}")))?;
    v.secrets = secrets;
    v.file = file;
    Ok(())
}

pub fn set_secret(key: &str, value: &str) -> Result<(), AppError> {
    mutate_vault(|_file, secrets| {
        secrets.insert(key.to_string(), value.to_string());
    })
}

pub fn remove_secret(key: &str) -> Result<(), AppError> {
    mutate_vault(|_file, secrets| {
        secrets.remove(key);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fast KDF params for tests (real vault uses much higher memory cost).
    fn test_kdf() -> KdfParams {
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        KdfParams {
            salt: B64.encode(salt),
            m_cost: 8,
            t_cost: 1,
            p_cost: 1,
        }
    }

    /// The full unlock path in memory: password wraps the DEK, DEK encrypts the
    /// secrets, and both come back intact.
    #[test]
    fn wrap_unwrap_round_trip() {
        let kdf = test_kdf();
        let mut dek = [0u8; 32];
        OsRng.fill_bytes(&mut dek);

        let mut secrets = BTreeMap::new();
        secrets.insert("profile-1".to_string(), "s3cr3t".to_string());
        secrets.insert("profile-1#ssh".to_string(), "passphrase".to_string());

        let kek = derive_kek("correct horse", &kdf).unwrap();
        let pw_wrap = encrypt(&kek, &dek).unwrap();
        let blob = encrypt(&dek, &serde_json::to_vec(&secrets).unwrap()).unwrap();

        // unlock: re-derive KEK, recover DEK, decrypt the secrets
        let kek2 = derive_kek("correct horse", &kdf).unwrap();
        let dek2 = decrypt(&kek2, &pw_wrap).unwrap();
        assert_eq!(dek2, dek);
        let plain = decrypt(&dek2.clone().try_into().unwrap(), &blob).unwrap();
        let recovered: BTreeMap<String, String> = serde_json::from_slice(&plain).unwrap();
        assert_eq!(recovered, secrets);
    }

    /// A wrong master password must fail to unwrap the DEK (GCM auth failure).
    #[test]
    fn wrong_password_fails() {
        let kdf = test_kdf();
        let mut dek = [0u8; 32];
        OsRng.fill_bytes(&mut dek);
        let kek = derive_kek("right", &kdf).unwrap();
        let pw_wrap = encrypt(&kek, &dek).unwrap();

        let wrong = derive_kek("wrong", &kdf).unwrap();
        assert!(decrypt(&wrong, &pw_wrap).is_err());
    }

    /// The KDF is deterministic for a given salt (so re-derivation works).
    #[test]
    fn kdf_is_deterministic() {
        let kdf = test_kdf();
        assert_eq!(
            derive_kek("pw", &kdf).unwrap(),
            derive_kek("pw", &kdf).unwrap()
        );
    }
}
