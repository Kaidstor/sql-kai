//! Пароли и vault: цепочка разлока и источники пароля БД помимо vault.

use std::io::IsTerminal;

use sql_kai_lib::error::AppError;
use sql_kai_lib::store::Profile;
use sql_kai_lib::vault;

use crate::envvar;

/// Мастер-пароль vault из окружения (см. `envvar` — единый префикс SQL_KAI_*).
fn master_password_env() -> Option<String> {
    envvar::value(envvar::VAULT_PASSWORD)
}

/// Тихая часть цепочки разлока (без TTY): keychain-trust -> SQL_KAI_VAULT_PASSWORD.
/// Holder поднимается только через неё — TTY у него нет по построению.
pub fn unlock_vault_headless() -> Result<(), AppError> {
    if vault::is_unlocked() {
        return Ok(());
    }
    if !vault::exists() {
        return Err(AppError::Msg(
            "vault не создан — `sql-kai vault setup` (или первый запуск GUI)".into(),
        ));
    }
    if vault::unlock_cli_trust().is_ok() {
        return Ok(());
    }
    if let Some(pw) = master_password_env() {
        return vault::unlock_password(&pw);
    }
    Err(AppError::Msg(format!(
        "vault заблокирован — настрой `sql-kai vault trust`, задай {} \
         или используй --password-env",
        envvar::VAULT_PASSWORD,
    )))
}

/// Спавнить holder имеет смысл, только если он сможет тихо разлочить vault —
/// иначе каждый `sql-kai q` плодил бы мгновенно умирающий процесс.
pub fn headless_unlock_possible() -> bool {
    vault::is_unlocked()
        || master_password_env().is_some()
        || (vault::exists() && vault::cli_trust_enrolled())
}

/// Полная цепочка разлока: тихая часть, затем запрос пароля в TTY.
pub fn unlock_vault() -> Result<(), AppError> {
    match unlock_vault_headless() {
        Ok(()) => Ok(()),
        Err(e) => {
            if vault::exists() && std::io::stdin().is_terminal() {
                let pw =
                    rpassword::prompt_password("vault master password: ").map_err(AppError::Io)?;
                return vault::unlock_password(&pw);
            }
            Err(e)
        }
    }
}

/// Разлочивает vault, только если профилю есть что из него читать.
pub(super) fn ensure_vault(profile: &Profile, have_override: bool) -> Result<(), AppError> {
    let needs = (profile.has_password && !have_override) || profile.has_ssh_passphrase;
    if !needs {
        return Ok(());
    }
    unlock_vault()
}

/// Мастер-пароль для нового vault: SQL_KAI_VAULT_PASSWORD или двойной ввод в TTY.
pub fn read_new_password() -> Result<String, AppError> {
    if let Some(pw) = master_password_env() {
        return Ok(pw);
    }
    if !std::io::stdin().is_terminal() {
        return Err(AppError::Msg(format!(
            "нет TTY — задай мастер-пароль через {}",
            envvar::VAULT_PASSWORD,
        )));
    }
    let pw = rpassword::prompt_password("новый мастер-пароль: ").map_err(AppError::Io)?;
    let again = rpassword::prompt_password("ещё раз: ").map_err(AppError::Io)?;
    if pw != again {
        return Err(AppError::Msg("пароли не совпадают".into()));
    }
    Ok(pw)
}

/// Источник пароля БД помимо vault: env-переменная или ключ sec.
#[derive(Default)]
pub struct PwSource<'a> {
    pub env: Option<&'a str>,
    pub from_sec: bool,
    pub sec_key: Option<&'a str>,
}

/// Разрешает пароль-override по приоритету: env → sec → (None = из vault).
pub(super) fn resolve_override(
    profile: &Profile,
    src: &PwSource,
) -> Result<Option<String>, AppError> {
    if let Some(v) = src.env {
        return std::env::var(v)
            .map(Some)
            .map_err(|_| AppError::Msg(format!("env-переменная {v} не задана")));
    }
    if src.from_sec || src.sec_key.is_some() {
        crate::sec::available()?;
        let key = src
            .sec_key
            .map(str::to_string)
            .unwrap_or_else(|| crate::sec::default_key(profile));
        return crate::sec::get(&key)?
            .map(Some)
            .ok_or_else(|| AppError::Msg(format!("в sec нет ключа {key}")));
    }
    Ok(None)
}
