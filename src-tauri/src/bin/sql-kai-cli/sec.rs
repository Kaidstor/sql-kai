//! Тонкая обёртка над CLI `sec` (менеджер секретов) — для тех команд sql-kai, что
//! кладут/читают пароли БД в sec, сверяют и сканируют. Значения всегда идут
//! через stdin/stdout, не через argv (агент-безопасно, как и весь sec).
//!
//! Бинарь: `KAI_SEC_BIN`, иначе `sec` из PATH. Требуется свежий sec (с
//! командами meta/scan/verify/--fingerprint).

use std::io::Write;
use std::process::{Command, Stdio};

use sql_kai_lib::error::AppError;
use sql_kai_lib::store::Profile;

fn sec_bin() -> String {
    std::env::var("KAI_SEC_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "sec".into())
}

/// Ключ sec для пароля БД профиля по конвенции: `<имя>/DB_PASSWORD`
/// ('/' в имени → '-', т.к. это разделитель proj/KEY).
pub fn default_key(profile: &Profile) -> String {
    let proj = profile.name.replace('/', "-");
    format!("{proj}/DB_PASSWORD")
}

fn base() -> Command {
    let mut cmd = Command::new(sec_bin());
    // мастер-пароль vault не должен наследоваться дочерними процессами
    cmd.env_remove("KAI_VAULT_PASSWORD");
    cmd
}

/// Проверка, что sec доступен и достаточно свежий (есть `scan`).
pub fn available() -> Result<(), AppError> {
    let out = base()
        .arg("--help")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| AppError::Msg(format!("sec не найден в PATH ({e}); установи sec или задай KAI_SEC_BIN")))?;
    let help = String::from_utf8_lossy(&out.stdout);
    if !help.contains("scan") {
        return Err(AppError::Msg(
            "sec в PATH устарел (нет команды scan) — пересобери его (`just install` в sec/cli)".into(),
        ));
    }
    Ok(())
}

/// `printf '%s' <value> | sec set <key> --stdin` — значение не через argv.
pub fn set(key: &str, value: &str) -> Result<(), AppError> {
    let mut child = base()
        .arg("set")
        .arg(key)
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::Msg(format!("sec set: {e}")))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(value.as_bytes())
        .map_err(AppError::Io)?;
    let out = child.wait_with_output().map_err(AppError::Io)?;
    if !out.status.success() {
        return Err(AppError::Msg(format!(
            "sec set {key}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// `sec get <key>` → значение (stdout). None, если ключа нет.
pub fn get(key: &str) -> Result<Option<String>, AppError> {
    let out = base()
        .arg("get")
        .arg(key)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| AppError::Msg(format!("sec get: {e}")))?;
    if !out.status.success() {
        return Ok(None);
    }
    let v = String::from_utf8_lossy(&out.stdout);
    Ok(Some(v.trim_end_matches('\n').to_string()))
}

/// `sec meta <key> …` — несекретные метаданные (best-effort, ошибки не валят).
pub fn meta(key: &str, rotate_every: Option<&str>, note: Option<&str>) {
    let mut cmd = base();
    cmd.arg("meta").arg(key);
    if let Some(r) = rotate_every {
        cmd.args(["--rotate-every", r]);
    }
    if let Some(n) = note {
        cmd.args(["--note", n]);
    }
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// `sec scan <path>` → (найдено_ли, строки отчёта). Exit 1 у sec = найдено.
pub fn scan(path: &str) -> Result<(bool, String), AppError> {
    let out = base()
        .arg("scan")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| AppError::Msg(format!("sec scan: {e}")))?;
    let report = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // exit 0 = чисто, 1 = найдены секреты; прочее = ошибка sec
    match out.status.code() {
        Some(0) => Ok((false, report)),
        Some(1) => Ok((true, report)),
        _ => Err(AppError::Msg(format!(
            "sec scan: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}
