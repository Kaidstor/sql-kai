//! `kai rotate <alias>` — ротация пароля роли Postgres, где sec и kai
//! складываются в то, чего не умеет ни один в одиночку: sec «сгенерировать +
//! сохранить + отследить срок + оставить старое в истории», kai «применить в
//! БД». sec здесь ещё и страховка от локаута — прежнее значение уходит в
//! `sec history`, так что сорванную ротацию можно откатить `sec undo`.
//!
//! Безопасный порядок: сохранить новый пароль в sec (старое → история) →
//! `ALTER ROLE … PASSWORD` → проверить свежим коннектом → только потом обновить
//! vault. vault получает пароль лишь после того, как он подтверждён рабочим.

use std::io::IsTerminal;
use std::process::ExitCode;

use rand::Rng;
use sql_tauri_lib::db;
use sql_tauri_lib::error::AppError;
use sql_tauri_lib::store;

use crate::{sec, session, RotateArgs};

/// base62 — совместим везде (URL/env/SQL), без экранирования.
fn gen_password(len: usize) -> String {
    const CS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..len).map(|_| CS[rng.gen_range(0..CS.len())] as char).collect()
}

pub async fn run(a: RotateArgs) -> Result<ExitCode, AppError> {
    // sec обязателен: он даёт откат старого значения при сбое.
    sec::available()?;

    // Коннект с текущими кредами (write — для ALTER); заодно резолвит профиль.
    let (profile, connected) = session::open_for(
        &a.alias,
        session::PwSource {
            env: a.password_env.as_deref(),
            from_sec: a.from_sec,
            sec_key: a.sec_key.as_deref(),
        },
        true,
        a.verbose,
        true,
    )
    .await?;

    let role = a.role.clone().unwrap_or_else(|| profile.user.clone());
    let key = a.sec_key.clone().unwrap_or_else(|| sec::default_key(&profile));

    eprintln!(
        "⚠ ротация пароля роли '{role}' в {}/{}. Если этой ролью ходит сам сервис — \
         он сломается, пока не обновишь его секрет (sec push → передеплой).",
        profile.name, profile.database
    );
    if !a.yes {
        if !std::io::stdin().is_terminal() {
            return Err(AppError::Msg("нет TTY для подтверждения — добавь --yes".into()));
        }
        eprint!("продолжить ротацию? [y/N] ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim(), "y" | "Y" | "yes") {
            eprintln!("отменено");
            return Ok(ExitCode::FAILURE);
        }
    }

    let new_pw = gen_password(a.length.clamp(8, 128));

    // 1) в sec (старое значение уедет в sec history — страховка).
    sec::set(&key, &new_pw)?;
    sec::meta(
        &key,
        a.rotate_every.as_deref().or(Some("90d")),
        Some(&format!("DB пароль {} (роль {role})", profile.name)),
    );
    eprintln!("новый пароль сохранён в sec: {key}");

    // 2) ALTER ROLE.
    let alter = format!(
        "ALTER ROLE {} WITH PASSWORD {}",
        db::quote_ident(&role),
        db::quote_literal(&new_pw)
    );
    if let Err(e) = db::execute(&connected.session.client, &alter, 1).await {
        eprintln!("kai: ALTER ROLE не прошёл: {e}");
        eprintln!("sec уже содержит новый пароль, а в БД он не применён — откати: sec undo {key}");
        return Ok(ExitCode::FAILURE);
    }
    drop(connected); // освободим соединение перед проверкой новым паролем

    // 3) проверка: свежий коннект новым паролем.
    let check = db::connect(
        &profile,
        db::ConnectOptions {
            password_override: Some(new_pw.clone()),
            ssh_mux_ttl: Some(300),
            ..Default::default()
        },
    )
    .await;
    if let Err(e) = check {
        eprintln!("kai: пароль сменён, но проверка коннекта не прошла: {e}");
        eprintln!("новый пароль в sec ({key}); восстановить старый — sec undo {key} (и повторно ALTER)");
        return Ok(ExitCode::FAILURE);
    }

    // 4) в vault — только теперь, когда пароль подтверждён рабочим.
    match session::unlock_vault() {
        Ok(()) => {
            store::upsert_profile(profile.clone(), Some(new_pw.clone()), None)?;
            println!("готово: пароль роли '{role}' обновлён; новый — в sec ({key}) и vault");
        }
        Err(_) => {
            println!(
                "готово: пароль роли '{role}' обновлён; новый — в sec ({key}). \
                 В vault не записан (заблокирован) — подключайся --from-sec или сделай vault trust"
            );
        }
    }
    println!("старое значение осталось в истории sec — откат: sec undo {key}");
    Ok(ExitCode::SUCCESS)
}
