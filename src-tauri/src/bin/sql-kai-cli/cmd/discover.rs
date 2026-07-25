//! `sql-kai discover <ssh-alias>` — найти postgres-контейнер на хосте, достать
//! креды из env контейнера и завести/обновить профиль (пароль — в vault).
//! Заменяет кеш-дискавери старого prod-db: результат — обычный профиль,
//! общий с GUI.

use std::process::ExitCode;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use clap::Args;
use sql_kai_lib::db;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, Profile, SshConfig};

use crate::remote::{self, CONTAINER_DB_ENV, CONTAINER_FIND};
use crate::{sec, session};

#[derive(Args)]
pub struct DiscoverArgs {
    /// SSH-алиас хоста (из ~/.ssh/config*)
    alias: String,
    /// Имя профиля (по умолчанию = alias)
    #[arg(long)]
    name: Option<String>,
    /// Docker-контейнер postgres (когда на хосте их несколько)
    #[arg(long, value_name = "NAME")]
    container: Option<String>,
    /// Положить найденный пароль в sec (по умолч. ключ <имя>/DB_PASSWORD)
    #[arg(long)]
    to_sec: bool,
    /// Ключ sec для пароля (proj/KEY); включает --to-sec
    #[arg(long, value_name = "PROJ/KEY")]
    sec_key: Option<String>,
    /// Не хранить пароль в vault (прод-политика: только sec / --password-env)
    #[arg(long)]
    no_vault: bool,
    /// Только показать найденное, не сохранять профиль
    #[arg(long)]
    dry_run: bool,
}

/// Хвост поверх [`CONTAINER_FIND`] + [`CONTAINER_DB_ENV`]: тянет POSTGRES_PASSWORD и решает, куда
/// туннелировать — опубликованный порт (docker port) или IP контейнера в
/// bridge-сети (с хоста маршрутизируется). Пароль — base64, чтобы спецсимволы
/// не ломали парсинг метастроки.
const DISCOVER_TAIL: &str = r#"# $(...) strips printenv's trailing newline before base64 — иначе пароль
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

fn discover_host(alias: &str, container: Option<&str>) -> Result<Discovered, AppError> {
    let script = format!("{CONTAINER_FIND}{CONTAINER_DB_ENV}{DISCOVER_TAIL}");
    let env = [(
        "KAI_CONTAINER",
        container.unwrap_or_default().to_string(),
    )];
    let out = remote::output_via_stdin(alias, &remote::stdin_payload(&script, &env))
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
            "у контейнера нет ни опубликованного порта, ни IP — используй `sql-kai exec`".into(),
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
    let d = discover_host(&a.alias, a.container.as_deref())?;
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
            keepalive_interval: None,
        }),
        group: existing.as_ref().and_then(|e| e.group.clone()),
        color: existing.as_ref().and_then(|e| e.color.clone()),
        production: existing.as_ref().map(|e| e.production).unwrap_or(false),
        ssl: existing.as_ref().and_then(|e| e.ssl.clone()),
        has_password: existing.as_ref().map(|e| e.has_password).unwrap_or(false),
        has_ssh_passphrase: existing
            .as_ref()
            .map(|e| e.has_ssh_passphrase)
            .unwrap_or(false),
        last_connected: None,
    };

    let to_sec = a.to_sec || a.sec_key.is_some();

    if a.dry_run {
        println!("dry-run: профиль не сохранён");
    } else {
        // 1) пароль в sec (если попросили и он найден)
        if to_sec {
            match &d.password {
                Some(pw) => {
                    sec::available()?;
                    let key = a
                        .sec_key
                        .clone()
                        .unwrap_or_else(|| sec::default_key(&profile));
                    sec::set(&key, pw)?;
                    sec::meta(&key, Some("90d"), Some(&format!("DB пароль {}", profile.name)));
                    println!("пароль сохранён в sec: {key}");
                }
                None => eprintln!("sql-kai: --to-sec задан, но пароль в env контейнера не найден"),
            }
        }
        // 2) пароль в vault (если не --no-vault)
        // None = не трогать существующий секрет (пароль не нашли/не кладём).
        let password_arg = match (&d.password, a.no_vault) {
            (Some(pw), false) => match session::unlock_vault() {
                Ok(()) => Some(pw.clone()),
                Err(e) => {
                    eprintln!(
                        "sql-kai: пароль найден, но в vault не сохранён ({e}); \
                         подключение потребует --password-env{}",
                        if to_sec { " или --from-sec" } else { "" }
                    );
                    None
                }
            },
            _ => None,
        };
        profile = store::upsert_profile(profile, password_arg, None)?;
        println!("профиль '{}' сохранён (id {})", profile.name, profile.id);
        crate::broker_client::notify_profiles_changed().await;
        if a.no_vault && to_sec {
            println!(
                "прод-режим: пароль только в sec — запускай `sql-kai {} -c \"…\" --from-sec` \
                 или `sec run {} -- …`",
                profile.name, profile.name
            );
        }
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
            eprintln!("sql-kai: подключение не удалось: {e}");
            eprintln!(
                "hint: если порт закрыт и IP контейнера недоступен — `sql-kai exec {} -c \"...\"`",
                a.alias
            );
            Ok(ExitCode::FAILURE)
        }
    }
}
