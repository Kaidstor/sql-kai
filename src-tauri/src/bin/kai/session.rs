//! Общий путь «алиас -> профиль -> vault -> соединение» для команд CLI.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use sql_tauri_lib::db;
use sql_tauri_lib::error::AppError;
use sql_tauri_lib::store::{self, Profile};
use sql_tauri_lib::vault;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// SQL из -c/-f, иначе со stdin (если это пайп).
pub fn collect_sql(commands: &[String], files: &[PathBuf]) -> Result<String, AppError> {
    let mut parts: Vec<String> = Vec::new();
    for c in commands {
        parts.push(format!("{};", c.trim_end().trim_end_matches(';')));
    }
    for f in files {
        let text = std::fs::read_to_string(f)
            .map_err(|e| AppError::Msg(format!("чтение {}: {e}", f.display())))?;
        parts.push(text);
    }
    if parts.is_empty() && !std::io::stdin().is_terminal() {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        if !s.trim().is_empty() {
            parts.push(s);
        }
    }
    let sql = parts.join("\n");
    if sql.trim().is_empty() {
        return Err(AppError::Msg(
            "нет SQL: передай -c \"...\", -f file.sql или подай на stdin".into(),
        ));
    }
    Ok(sql)
}

/// Алиас = id, имя или группа профиля (имя/группа — без учёта регистра).
pub fn resolve_profile(alias: &str) -> Result<Profile, AppError> {
    let all = store::load_profiles()?;
    if let Some(p) = all.iter().find(|p| p.id == alias) {
        return Ok(p.clone());
    }
    let by_name: Vec<&Profile> = all
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(alias))
        .collect();
    if by_name.len() == 1 {
        return Ok(by_name[0].clone());
    }
    if by_name.len() > 1 {
        return Err(AppError::Msg(format!(
            "несколько профилей с именем '{alias}' — укажи id (kai profiles list)"
        )));
    }
    let by_group: Vec<&Profile> = all
        .iter()
        .filter(|p| {
            p.group
                .as_deref()
                .map(|g| g.eq_ignore_ascii_case(alias))
                .unwrap_or(false)
        })
        .collect();
    match by_group.len() {
        1 => Ok(by_group[0].clone()),
        0 => {
            let known = all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
            Err(AppError::Msg(format!(
                "профиль '{alias}' не найден (есть: {known}); для нового прод-хоста: kai discover {alias}"
            )))
        }
        _ => {
            let names = by_group.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
            Err(AppError::Msg(format!(
                "в группе '{alias}' несколько профилей — уточни: {names}"
            )))
        }
    }
}

pub fn password_override(var: Option<&str>) -> Result<Option<String>, AppError> {
    match var {
        None => Ok(None),
        Some(v) => std::env::var(v)
            .map(Some)
            .map_err(|_| AppError::Msg(format!("env-переменная {v} не задана"))),
    }
}

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
pub fn ensure_vault(profile: &Profile, have_override: bool) -> Result<(), AppError> {
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

/// TTL персистентного ssh-мастера (сек): env KAI_SSH_MUX_TTL, иначе 5 минут.
fn mux_ttl() -> u32 {
    std::env::var("KAI_SSH_MUX_TTL")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(300)
}

/// Подключение к профилю; без `write` сессия сразу переводится в read-only.
/// `mux` включает переиспользование ssh-туннеля через ControlMaster.
pub async fn open(
    profile: &Profile,
    password_override: Option<String>,
    write: bool,
    verbose: bool,
    mux: bool,
) -> Result<db::Connected, AppError> {
    let connected = db::connect(
        profile,
        db::ConnectOptions {
            password_override,
            ssh_mux_ttl: if mux { Some(mux_ttl()) } else { None },
            ..Default::default()
        },
    )
    .await?;
    if !write {
        db::execute(
            &connected.session.client,
            "SET default_transaction_read_only = on",
            1,
        )
        .await?;
    }
    if verbose {
        let via = connected
            .tunnel_port
            .map(|p| format!(" via ssh:{p}"))
            .unwrap_or_default();
        eprintln!(
            "[{} -> {}@{}:{}/{}{} PostgreSQL {} {}]",
            profile.name,
            profile.user,
            profile.host,
            profile.port,
            profile.database,
            via,
            connected.server_version,
            if write { "rw" } else { "ro" }
        );
    }
    Ok(connected)
}

/// Полный путь для команд: резолв алиаса, vault, соединение.
pub async fn open_for(
    alias: &str,
    password_env: Option<&str>,
    write: bool,
    verbose: bool,
    mux: bool,
) -> Result<(Profile, db::Connected), AppError> {
    let profile = resolve_profile(alias)?;
    let pw = password_override(password_env)?;
    ensure_vault(&profile, pw.is_some())?;
    let connected = open(&profile, pw, write, verbose, mux).await?;
    Ok((profile, connected))
}
