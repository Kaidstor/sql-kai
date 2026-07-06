//! `kai discover <ssh-alias>` — найти postgres-контейнер на хосте, достать
//! креды из env контейнера и завести/обновить профиль (пароль — в vault).
//! Заменяет кеш-дискавери старого prod-db: результат — обычный профиль,
//! общий с GUI.

use std::process::{ExitCode, Stdio};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use sql_tauri_lib::db;
use sql_tauri_lib::error::AppError;
use sql_tauri_lib::store::{self, Profile, SshConfig};

use crate::remote::{remote_command, ssh_base};
use crate::{session, DiscoverArgs};

/// Ищет db-контейнер, тянет POSTGRES_USER/DB/PASSWORD и решает, куда
/// туннелировать: опубликованный порт (docker port) или IP контейнера в
/// bridge-сети (с хоста маршрутизируется). Пароль — base64, чтобы спецсимволы
/// не ломали парсинг метастроки.
const REMOTE_SCRIPT: &str = r#"set -u
D=docker
docker ps >/dev/null 2>&1 || D="sudo docker"
C=$($D ps --format '{{.Names}}' 2>/dev/null | grep -iE '(^|[-_])db([-_]|$)|postgres' | head -1)
[ -n "$C" ] || { echo 'kai: postgres-контейнер не найден в docker ps' >&2; exit 3; }
U=$($D exec "$C" printenv POSTGRES_USER 2>/dev/null)
[ -n "$U" ] || U=postgres
DB=$($D exec "$C" printenv POSTGRES_DB 2>/dev/null)
[ -n "$DB" ] || DB=$($D exec "$C" psql -U "$U" -tAc "SELECT datname FROM pg_database WHERE datname NOT IN ('postgres','template0','template1') ORDER BY datname LIMIT 1" 2>/dev/null | tr -d '[:space:]')
[ -n "$DB" ] || { echo 'kai: не удалось определить имя базы' >&2; exit 5; }
# $(...) strips printenv's trailing newline before base64 — иначе пароль
# приедет с лишним \n и TCP-аутентификация не пройдёт.
PWV=$($D exec "$C" printenv POSTGRES_PASSWORD 2>/dev/null)
PW=$(printf '%s' "$PWV" | base64 | tr -d '\n')
PORT=$($D port "$C" 5432/tcp 2>/dev/null | head -1 | sed 's/.*://')
IP=$($D inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}' "$C" 2>/dev/null | awk '{print $1}')
printf '__KAI__ container=%s user=%s db=%s port=%s ip=%s pw=%s\n' "$C" "$U" "$DB" "$PORT" "$IP" "$PW"
"#;

struct Discovered {
    container: String,
    user: String,
    database: String,
    password: Option<String>,
    /// (host, port) с точки зрения ssh-хоста — цель для `ssh -L`.
    endpoint: (String, u16),
}

fn discover_host(alias: &str) -> Result<Discovered, AppError> {
    let out = ssh_base(alias)
        .arg(remote_command(REMOTE_SCRIPT, &[]))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| AppError::Msg(format!("ssh: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if !line.trim().is_empty() {
            eprintln!("{line}");
        }
    }
    let meta = stdout
        .lines()
        .find(|l| l.starts_with("__KAI__ "))
        .ok_or_else(|| {
            AppError::Msg(format!(
                "discovery не вернул метаданные (ssh exit {})",
                out.status.code().unwrap_or(-1)
            ))
        })?;

    let mut container = String::new();
    let mut user = String::new();
    let mut database = String::new();
    let mut port = String::new();
    let mut ip = String::new();
    let mut pw_b64 = String::new();
    for kv in meta.trim_start_matches("__KAI__ ").split_whitespace() {
        if let Some((k, v)) = kv.split_once('=') {
            match k {
                "container" => container = v.into(),
                "user" => user = v.into(),
                "db" => database = v.into(),
                "port" => port = v.into(),
                "ip" => ip = v.into(),
                "pw" => pw_b64 = v.into(),
                _ => {}
            }
        }
    }

    let password = if pw_b64.is_empty() {
        None
    } else {
        let raw = B64
            .decode(&pw_b64)
            .map_err(|e| AppError::Msg(format!("не смог раскодировать пароль: {e}")))?;
        String::from_utf8(raw)
            .ok()
            .filter(|p| !p.is_empty())
    };

    let endpoint = if let Ok(p) = port.parse::<u16>() {
        ("127.0.0.1".to_string(), p)
    } else if !ip.is_empty() {
        (ip, 5432)
    } else {
        return Err(AppError::Msg(
            "у контейнера нет ни опубликованного порта, ни IP — используй `kai exec`".into(),
        ));
    };

    Ok(Discovered {
        container,
        user,
        database,
        password,
        endpoint,
    })
}

pub async fn run(a: DiscoverArgs) -> Result<ExitCode, AppError> {
    eprintln!("[{}] discovery…", a.alias);
    let d = discover_host(&a.alias)?;
    eprintln!(
        "[container={} user={} db={} endpoint={}:{} password={}]",
        d.container,
        d.user,
        d.database,
        d.endpoint.0,
        d.endpoint.1,
        if d.password.is_some() { "найден" } else { "нет" }
    );

    let name = a.name.clone().unwrap_or_else(|| a.alias.clone());
    let existing = store::load_profiles()?
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(&name));
    let mut profile = Profile {
        id: existing.as_ref().map(|e| e.id.clone()).unwrap_or_default(),
        name,
        host: d.endpoint.0.clone(),
        port: d.endpoint.1,
        database: d.database.clone(),
        user: d.user.clone(),
        ssh: Some(SshConfig {
            host: a.alias.clone(),
            user: None,
            port: None,
            key_path: None,
        }),
        group: existing.as_ref().and_then(|e| e.group.clone()),
        color: existing.as_ref().and_then(|e| e.color.clone()),
        has_password: existing.as_ref().map(|e| e.has_password).unwrap_or(false),
        has_ssh_passphrase: existing
            .as_ref()
            .map(|e| e.has_ssh_passphrase)
            .unwrap_or(false),
    };

    if a.dry_run {
        println!("dry-run: профиль не сохранён");
    } else {
        // None = не трогать существующий секрет (пароль в env не нашли).
        let password_arg = match &d.password {
            Some(pw) => match session::unlock_vault() {
                Ok(()) => Some(pw.clone()),
                Err(e) => {
                    eprintln!(
                        "kai: пароль найден, но в vault не сохранён ({e}); \
                         подключение потребует --password-env"
                    );
                    None
                }
            },
            None => None,
        };
        profile = store::upsert_profile(profile, password_arg, None)?;
        println!("профиль '{}' сохранён (id {})", profile.name, profile.id);
    }

    eprintln!("проверка подключения…");
    let check = db::connect(
        &profile,
        db::ConnectOptions {
            password_override: d.password.clone(),
            ..Default::default()
        },
    )
    .await;
    match check {
        Ok(c) => {
            println!("ok: PostgreSQL {}", c.server_version);
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            eprintln!("kai: подключение не удалось: {e}");
            eprintln!(
                "hint: если порт закрыт и IP контейнера недоступен — `kai exec {} -c \"...\"`",
                a.alias
            );
            Ok(ExitCode::FAILURE)
        }
    }
}
