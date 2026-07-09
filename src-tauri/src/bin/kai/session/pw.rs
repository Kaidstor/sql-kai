//! Пароли и vault: цепочка разлока и источники пароля БД помимо vault.

use std::io::IsTerminal;

use sql_kai_lib::error::AppError;
use sql_kai_lib::store::Profile;
use sql_kai_lib::vault;

/// Цепочка разлока vault: keychain-trust -> KAI_VAULT_PASSWORD -> запрос в TTY.
pub fn unlock_vault() -> Result<(), AppError> {
    if vault::is_unlocked() {
        return Ok(());
    }
    if !vault::exists() {
        return Err(AppError::Msg(
            "vault не создан — `kai vault setup` (или первый запуск GUI)".into(),
        ));
    }
    if vault::unlock_cli_trust().is_ok() {
        return Ok(());
    }
    if let Ok(pw) = std::env::var("KAI_VAULT_PASSWORD") {
        if !pw.is_empty() {
            return vault::unlock_password(&pw);
        }
    }
    if std::io::stdin().is_terminal() {
        let pw = rpassword::prompt_password("vault master password: ").map_err(AppError::Io)?;
        return vault::unlock_password(&pw);
    }
    Err(AppError::Msg(
        "vault заблокирован — настрой `kai vault trust`, задай KAI_VAULT_PASSWORD \
         или используй --password-env"
            .into(),
    ))
}

/// Разлочивает vault, только если профилю есть что из него читать.
pub(super) fn ensure_vault(profile: &Profile, have_override: bool) -> Result<(), AppError> {
    let needs = (profile.has_password && !have_override) || profile.has_ssh_passphrase;
    if !needs {
        return Ok(());
    }
    unlock_vault()
}

/// Мастер-пароль для нового vault: KAI_VAULT_PASSWORD или двойной ввод в TTY.
pub fn read_new_password() -> Result<String, AppError> {
    if let Ok(pw) = std::env::var("KAI_VAULT_PASSWORD") {
        if !pw.is_empty() {
            return Ok(pw);
        }
    }
    if !std::io::stdin().is_terminal() {
        return Err(AppError::Msg(
            "нет TTY — задай мастер-пароль через KAI_VAULT_PASSWORD".into(),
        ));
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
pub(super) fn resolve_override(profile: &Profile, src: &PwSource) -> Result<Option<String>, AppError> {
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
