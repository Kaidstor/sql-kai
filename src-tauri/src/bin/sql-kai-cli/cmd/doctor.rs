//! `sql-kai doctor` — здоровье соединений: для каждого профиля проверяет, что
//! сохранённый пароль ещё аутентифицируется, и, если пароль дублируется в sec,
//! ловит дрейф (в vault одно, в sec другое, в БД работает третье).

use std::process::ExitCode;

use clap::Args;
use sql_kai_lib::db;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, Profile};

use crate::output::{self, Format, FormatArgs};
use crate::{sec, session};

#[derive(Args)]
pub struct DoctorArgs {
    /// Проверить один профиль (имя или id); без него — все
    alias: Option<String>,
    #[command(flatten)]
    fmt: FormatArgs,
}

/// Итог проверки одного источника пароля.
fn probe_label(res: &Result<(), AppError>) -> &'static str {
    if res.is_ok() {
        "ok"
    } else {
        "fail"
    }
}

async fn can_connect(profile: &Profile, password: Option<String>) -> Result<(), AppError> {
    let c = db::connect(
        profile,
        db::ConnectOptions {
            password_override: password,
            ssh_mux_ttl: Some(session::mux_ttl()),
            ..Default::default()
        },
    )
    .await?;
    drop(c);
    Ok(())
}

pub async fn run(a: DoctorArgs) -> Result<ExitCode, AppError> {
    let mut profiles = store::load_profiles()?;
    if let Some(alias) = &a.alias {
        // Единый резолв с `sql-kai q`: id, имя или группа.
        profiles = session::filter_profiles(&profiles, alias);
        if profiles.is_empty() {
            return Err(AppError::Msg(format!("профиль '{alias}' не найден")));
        }
    }
    // Разлочим vault best-effort, чтобы проверить vault-пароли; не вышло — просто
    // отметим их как locked.
    let vault_ok = session::unlock_vault().is_ok();
    let sec_ok = sec::available().is_ok();

    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut any_problem = false;

    for p in &profiles {
        // vault
        let vault_state = if !p.has_password {
            "no-pw".to_string()
        } else if !vault_ok {
            "locked".to_string()
        } else {
            probe_label(&can_connect(p, None).await).to_string()
        };

        // sec (по конвенционному ключу)
        let key = sec::default_key(p);
        let sec_value = if sec_ok { sec::get(&key).ok().flatten() } else { None };
        let sec_state = match &sec_value {
            None => "absent".to_string(),
            Some(v) => probe_label(&can_connect(p, Some(v.clone())).await).to_string(),
        };

        // "fail" бывает только когда пароль реально был и не подошёл — это и есть
        // проблема (дрейф/протухший креденшел); absent/no-pw/locked/ok — нет.
        let problem = vault_state == "fail" || sec_state == "fail";
        if problem {
            any_problem = true;
        }
        let note = match (vault_state.as_str(), sec_state.as_str()) {
            ("ok", _) => "",
            ("fail", "ok") => "дрейф: vault не подходит, работает sec",
            ("fail", _) => "vault-пароль не подходит",
            ("no-pw", "ok") => "sec-only ok",
            ("no-pw", "fail") => "sec-пароль не подходит",
            ("no-pw", "absent") => "нет сохранённого пароля — только --password-env",
            ("locked", "ok") => "vault заблокирован, работает sec",
            ("locked", _) => "vault заблокирован — проверить нельзя",
            _ => "",
        };

        rows.push(serde_json::json!({
            "name": p.name,
            "vault": vault_state,
            "sec": sec_state,
            "note": note,
        }));
    }

    let fmt = a.fmt.pick();
    if fmt == Format::Json {
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
    } else {
        let table: Vec<Vec<Option<String>>> = rows
            .iter()
            .map(|r| {
                vec![
                    r["name"].as_str().map(str::to_string),
                    r["vault"].as_str().map(str::to_string),
                    r["sec"].as_str().map(str::to_string),
                    r["note"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
                ]
            })
            .collect();
        output::print_rows(&["profile", "vault", "sec", "note"], &table, fmt);
        if sec_ok && fmt == Format::Table {
            eprintln!("подсказка: что пора ротировать по срокам — `sec stale`");
        }
    }

    Ok(if any_problem {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
