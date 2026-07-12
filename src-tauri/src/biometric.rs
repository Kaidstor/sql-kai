//! Touch ID fast path for the vault DEK (macOS only).
//!
//! Implementation: the app itself runs the biometry check via
//! `LocalAuthentication` (`LAContext`), and only after it succeeds reads the
//! DEK from a regular login-keychain item. Unlike an access-controlled
//! data-protection keychain item this needs **no entitlements, provisioning
//! profiles or Apple developer membership**, so it works in every build —
//! including `tauri dev`. The trade-off: biometry is enforced by the app, not
//! by the keychain item itself; at rest the DEK is protected by the login
//! keychain encryption plus its per-app ACL (signed builds keep a stable
//! code signature, so there are no repeat ACL prompts).
//!
//! The master password always remains the app-level fallback.

/// Outcomes the vault must handle differently from generic failures.
pub enum BioError {
    /// The user dismissed the system prompt — not an error to display.
    Cancelled,
    /// The stored key is missing/unreadable — disable and re-enroll.
    Stale,
    Other(String),
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};
    use security_framework::passwords;

    use super::BioError;

    const SERVICE: &str = "com.kaidstor.sql-kai.vault";
    /// Имя сервиса до ребрендинга v1.0 — читается как fallback и переносится
    /// под новое имя при первом обращении.
    const SERVICE_LEGACY: &str = "com.kaidstor.sql-tauri.vault";
    const ACCOUNT: &str = "dek";
    /// DEK copy for the `sql-kai` CLI: same login keychain, but read without a
    /// biometry gate — the keychain's per-app ACL is the only guard, so short
    /// CLI invocations stay prompt-free after a one-time "Always Allow".
    const ACCOUNT_CLI: &str = "dek-cli";

    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
    /// LAError codes meaning "the user chose not to proceed":
    /// userCancel (-2), userFallback (-3), systemCancel (-4), appCancel (-9).
    const LA_CANCEL_CODES: [isize; 4] = [-2, -3, -4, -9];

    /// True when actual biometrics (Touch ID) are available and enrolled.
    pub fn supported() -> bool {
        let ctx = unsafe { LAContext::new() };
        unsafe { ctx.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics) }
            .is_ok()
    }

    /// Blocks on the system authentication sheet (Touch ID, with the account
    /// password as the built-in system fallback). Call off the main thread.
    fn authenticate(reason: &str) -> Result<(), BioError> {
        let ctx = unsafe { LAContext::new() };
        let (tx, rx) = mpsc::channel::<Result<(), (isize, String)>>();
        let block = RcBlock::new(move |success: Bool, error: *mut NSError| {
            let outcome = if success.as_bool() {
                Ok(())
            } else if error.is_null() {
                Err((0, "authentication failed".to_string()))
            } else {
                let e = unsafe { &*error };
                Err((e.code(), e.localizedDescription().to_string()))
            };
            let _ = tx.send(outcome);
        });
        unsafe {
            ctx.evaluatePolicy_localizedReason_reply(
                LAPolicy::DeviceOwnerAuthentication,
                &NSString::from_str(reason),
                &block,
            );
        }
        // `ctx` must stay alive until the reply lands (dropping it cancels).
        match rx.recv() {
            Ok(Ok(())) => Ok(()),
            Ok(Err((code, _))) if LA_CANCEL_CODES.contains(&code) => Err(BioError::Cancelled),
            Ok(Err((_, msg))) => Err(BioError::Other(msg)),
            Err(_) => Err(BioError::Other("authentication callback was dropped".into())),
        }
    }

    /// Stores the DEK in the login keychain (replaces any previous copy).
    pub fn store_dek(dek: &[u8]) -> Result<(), BioError> {
        passwords::set_generic_password(SERVICE, ACCOUNT, dek)
            .map_err(|e| BioError::Other(format!("keychain error: {e}")))
    }

    /// Reads `account`, transparently migrating an item stored under the
    /// pre-1.0 service name to the current one.
    fn get_with_migration(account: &str) -> Result<Vec<u8>, BioError> {
        match passwords::get_generic_password(SERVICE, account) {
            Ok(v) => Ok(v),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => {
                let v = passwords::get_generic_password(SERVICE_LEGACY, account).map_err(|e| {
                    if e.code() == ERR_SEC_ITEM_NOT_FOUND {
                        BioError::Stale
                    } else {
                        BioError::Other(format!("keychain error: {e}"))
                    }
                })?;
                if passwords::set_generic_password(SERVICE, account, &v).is_ok() {
                    let _ = passwords::delete_generic_password(SERVICE_LEGACY, account);
                }
                Ok(v)
            }
            Err(e) => Err(BioError::Other(format!("keychain error: {e}"))),
        }
    }

    /// Touch ID gate, then the keychain read.
    pub fn read_dek() -> Result<Vec<u8>, BioError> {
        authenticate("unlock your saved database passwords")?;
        get_with_migration(ACCOUNT)
    }

    /// Best-effort removal of the stored DEK (both on disable and re-enroll).
    pub fn delete_dek() {
        let _ = passwords::delete_generic_password(SERVICE, ACCOUNT);
        let _ = passwords::delete_generic_password(SERVICE_LEGACY, ACCOUNT);
    }

    pub fn store_dek_cli(dek: &[u8]) -> Result<(), BioError> {
        passwords::set_generic_password(SERVICE, ACCOUNT_CLI, dek)
            .map_err(|e| BioError::Other(format!("keychain error: {e}")))
    }

    /// Keychain read without an app-level auth gate (see ACCOUNT_CLI).
    pub fn read_dek_cli() -> Result<Vec<u8>, BioError> {
        get_with_migration(ACCOUNT_CLI)
    }

    pub fn delete_dek_cli() {
        let _ = passwords::delete_generic_password(SERVICE, ACCOUNT_CLI);
        let _ = passwords::delete_generic_password(SERVICE_LEGACY, ACCOUNT_CLI);
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::BioError;

    pub fn supported() -> bool {
        false
    }

    pub fn store_dek(_dek: &[u8]) -> Result<(), BioError> {
        Err(BioError::Other(
            "biometric unlock is not supported on this OS".into(),
        ))
    }

    pub fn read_dek() -> Result<Vec<u8>, BioError> {
        Err(BioError::Other(
            "biometric unlock is not supported on this OS".into(),
        ))
    }

    pub fn delete_dek() {}

    pub fn store_dek_cli(_dek: &[u8]) -> Result<(), BioError> {
        Err(BioError::Other(
            "keychain trust is not supported on this OS".into(),
        ))
    }

    pub fn read_dek_cli() -> Result<Vec<u8>, BioError> {
        Err(BioError::Other(
            "keychain trust is not supported on this OS".into(),
        ))
    }

    pub fn delete_dek_cli() {}
}

pub use imp::{
    delete_dek, delete_dek_cli, read_dek, read_dek_cli, store_dek, store_dek_cli, supported,
};
