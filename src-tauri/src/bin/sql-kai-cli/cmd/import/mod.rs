//! `sql-kai import` — массовый импорт профилей. Два входа в одну и ту же
//! запись: нейтральный JSON (stdin или файл, по объекту на подключение) и
//! `--from beekeeper|dbeaver` — чтение чужого клиента прямо из его файлов.
//! Пароли и ssh-passphrase кладутся в vault, в вывод не попадают никогда.

mod beekeeper;
mod dbeaver;

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Deserialize;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, Profile, SshConfig, SslConfig};

use crate::session;

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum Source {
    /// ~/Library/Application Support/beekeeper-studio/app.db (или --file)
    Beekeeper,
    /// ~/Library/DBeaverData/workspace6/*/.dbeaver/data-sources.json (или --file)
    Dbeaver,
}

#[derive(clap::Args)]
pub struct ImportArgs {
    /// Читать подключения из другого клиента вместо JSON
    #[arg(long, value_enum)]
    from: Option<Source>,
    /// Без --from: файл с JSON-массивом профилей (по умолчанию — stdin).
    /// С --from: файл или каталог источника, если он лежит не по умолчанию
    #[arg(short, long)]
    file: Option<PathBuf>,
    /// Перезаписывать профиль с таким же именем (иначе — пропускать)
    #[arg(long)]
    replace: bool,
    /// Ничего не писать, только показать план
    #[arg(long)]
    dry_run: bool,
    /// Не переносить пароли и passphrase — только параметры подключений
    #[arg(long)]
    no_passwords: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportSsh {
    host: String,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    key_path: Option<String>,
    #[serde(default)]
    keepalive_interval: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportProfile {
    name: String,
    host: String,
    port: u16,
    database: String,
    user: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    ssh: Option<ImportSsh>,
    #[serde(default)]
    ssh_passphrase: Option<String>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    production: bool,
    #[serde(default)]
    ssl: Option<SslConfig>,
}

/// Что удалось разобрать в источнике и о чём предупредить: подключение не к
/// postgres, ssh-режим без аналога в sql-kai, нерасшифрованный пароль.
#[derive(Default)]
struct Imported {
    profiles: Vec<ImportProfile>,
    notes: Vec<String>,
}

/// Пустая строка секрета = None (не трогать), непустая = положить в vault.
fn some_nonempty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

fn read_json(file: Option<&PathBuf>) -> Result<Vec<ImportProfile>, AppError> {
    let raw = match file {
        Some(path) => std::fs::read_to_string(path)?,
        None => {
            if std::io::stdin().is_terminal() {
                return Err(AppError::Msg(
                    "нет входных данных: передай JSON в stdin, через --file или укажи --from".into(),
                ));
            }
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };
    serde_json::from_str(&raw).map_err(|e| AppError::Msg(format!("не разобрать JSON профилей: {e}")))
}

pub async fn run(a: ImportArgs) -> Result<ExitCode, AppError> {
    let Imported {
        mut profiles,
        notes,
    } = match a.from {
        Some(Source::Beekeeper) => beekeeper::read(a.file.as_deref(), !a.no_passwords)?,
        Some(Source::Dbeaver) => dbeaver::read(a.file.as_deref(), !a.no_passwords)?,
        None => Imported {
            profiles: read_json(a.file.as_ref())?,
            notes: Vec::new(),
        },
    };
    if a.no_passwords {
        for p in &mut profiles {
            p.password = None;
            p.ssh_passphrase = None;
        }
    }

    for note in &notes {
        println!("! {note}");
    }
    if !notes.is_empty() {
        println!();
    }
    if profiles.is_empty() {
        println!("нет профилей для импорта");
        return Ok(ExitCode::SUCCESS);
    }

    // Разлочиваем vault один раз, если есть что в него класть.
    let needs_vault = profiles.iter().any(|p| {
        some_nonempty(p.password.clone()).is_some()
            || some_nonempty(p.ssh_passphrase.clone()).is_some()
    });
    if needs_vault && !a.dry_run {
        session::unlock_vault()?;
    }

    let existing = store::load_profiles()?;
    let (mut created, mut replaced, mut skipped) = (0u32, 0u32, 0u32);

    for imp in profiles {
        let existing_id = existing.iter().find(|p| p.name == imp.name).map(|p| p.id.clone());
        let action = match (&existing_id, a.replace) {
            (Some(_), false) => {
                println!("• пропуск (уже есть): {}", imp.name);
                skipped += 1;
                continue;
            }
            (Some(_), true) => "replace",
            (None, _) => "create",
        };

        let ssh = imp.ssh.map(|s| SshConfig {
            host: s.host.trim().to_string(),
            user: s.user.map(|u| u.trim().to_string()),
            port: s.port,
            key_path: s.key_path,
            keepalive_interval: s.keepalive_interval,
        });
        let profile = Profile {
            id: existing_id.clone().unwrap_or_default(),
            name: imp.name.clone(),
            host: imp.host,
            port: imp.port,
            database: imp.database,
            user: imp.user,
            ssh,
            group: imp.group.filter(|g| !g.trim().is_empty()),
            color: imp.color.filter(|c| !c.trim().is_empty()),
            production: imp.production,
            ssl: imp.ssl,
            fork_container: None,
            has_password: false,
            has_ssh_passphrase: false,
            last_connected: None,
        };

        let pw = some_nonempty(imp.password);
        let pp = some_nonempty(imp.ssh_passphrase);
        let has_pw = pw.is_some();

        if a.dry_run {
            println!(
                "• {action}: {} ({}:{}/{}, ssh={}, pw={})",
                profile.name,
                profile.host,
                profile.port,
                profile.database,
                profile.ssh.as_ref().map(|s| s.host.as_str()).unwrap_or("-"),
                if has_pw { "да" } else { "нет" },
            );
        } else {
            store::upsert_profile(profile, pw, pp)?;
            println!("• {action}: {}", imp.name);
        }

        match action {
            "replace" => replaced += 1,
            _ => created += 1,
        }
    }

    println!(
        "\n{}: создано {created}, заменено {replaced}, пропущено {skipped}",
        if a.dry_run { "план (dry-run)" } else { "готово" }
    );
    if !a.dry_run && (created > 0 || replaced > 0) {
        crate::broker_client::notify_profiles_changed().await;
    }
    Ok(ExitCode::SUCCESS)
}
