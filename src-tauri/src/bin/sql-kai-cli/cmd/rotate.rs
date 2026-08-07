//! `sql-kai rotate <alias>` — ротация пароля роли Postgres, где sec и sql-kai
//! складываются в то, чего не умеет ни один в одиночку: sec «сгенерировать +
//! сохранить + отследить срок + оставить старое в истории», sql-kai «применить в
//! БД». sec здесь ещё и страховка от локаута — прежнее значение уходит в
//! `sec history`, так что сорванную ротацию можно откатить `sec undo`.
//!
//! Безопасный порядок: сохранить новый пароль в sec (старое → история) →
//! `ALTER ROLE … PASSWORD` → проверить свежим коннектом → только потом обновить
//! vault. vault получает пароль лишь после того, как он подтверждён рабочим.

use std::process::ExitCode;

use clap::Args;
use rand::Rng;
use sql_kai_lib::db;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store;

use crate::{sec, session};

#[derive(Args)]
pub struct RotateArgs {
    /// Профиль: имя, id или группа
    alias: String,
    /// Роль Postgres (по умолчанию — пользователь профиля)
    #[arg(long)]
    role: Option<String>,
    /// Ключ sec для пароля (proj/KEY); по умолчанию <имя>/DB_PASSWORD
    #[arg(long, value_name = "PROJ/KEY")]
    sec_key: Option<String>,
    /// Длина нового пароля
    #[arg(long, default_value_t = 32)]
    length: usize,
    /// Интервал ротации для sec meta (напр. 90d)
    #[arg(long, value_name = "DUR")]
    rotate_every: Option<String>,
    /// Текущий пароль из env-переменной (если не в vault/sec)
    #[arg(long, value_name = "VAR")]
    password_env: Option<String>,
    /// Текущий пароль взять из sec
    #[arg(long)]
    from_sec: bool,
    /// Не спрашивать подтверждение
    #[arg(long)]
    yes: bool,
    /// Подтвердить запись в production-профиль (ALTER ROLE — запись в БД)
    #[arg(long)]
    prod_write: bool,
    #[arg(short, long)]
    verbose: bool,
}

/// base62 — совместим везде (URL/env/SQL), без экранирования.
fn gen_password(len: usize) -> String {
    const CS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CS[rng.gen_range(0..CS.len())] as char)
        .collect()
}

pub async fn run(a: RotateArgs) -> Result<ExitCode, AppError> {
    // sec обязателен: он даёт откат старого значения при сбое.
    sec::available()?;

    // ALTER ROLE — запись в БД, значит для прод-профиля нужен тот же барьер,
    // что и у `q --write`. Резолвим профиль заранее: барьер должен сработать
    // до коннекта, а `--yes` его не заменяет (это подтверждение ротации, а не
    // разрешение писать в прод).
    session::authorize_prod_write(&session::resolve_profile(&a.alias)?, a.prod_write)?;

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
    let key = a
        .sec_key
        .clone()
        .unwrap_or_else(|| sec::default_key(&profile));

    eprintln!(
        "⚠ ротация пароля роли '{role}' в {}/{}. Если этой ролью ходит сам сервис — \
         он сломается, пока не обновишь его секрет (sec push → передеплой).",
        profile.name, profile.database
    );
    if !a.yes && !crate::input::confirm("продолжить ротацию?", "--yes")? {
        eprintln!("отменено");
        return Ok(ExitCode::FAILURE);
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

    // 2) ALTER ROLE. На сервер уходит SCRAM-verifier, а не сам пароль —
    // иначе plaintext оседает в server log (log_statement=ddl/all) и
    // pg_stat_activity, а без ssh-туннеля ещё и в проводе (NoTls).
    let verifier = postgres_protocol::password::scram_sha_256(new_pw.as_bytes());
    let alter = format!(
        "ALTER ROLE {} WITH PASSWORD {}",
        db::quote_ident(&role),
        db::quote_literal(&verifier)
    );
    if let Err(e) = db::execute(&connected.session.client, &alter, 1).await {
        eprintln!("sql-kai: ALTER ROLE не прошёл: {e}");
        eprintln!("sec уже содержит новый пароль, а в БД он не применён — откати: sec undo {key}");
        return Ok(ExitCode::FAILURE);
    }
    drop(connected); // освободим соединение перед проверкой новым паролем

    // 3) проверка: свежий коннект новым паролем.
    let check = db::connect(
        &profile,
        db::ConnectOptions {
            password_override: Some(new_pw.clone()),
            ssh_mux_ttl: Some(crate::session::mux_ttl()),
            ..Default::default()
        },
    )
    .await;
    if let Err(e) = check {
        eprintln!("sql-kai: пароль сменён, но проверка коннекта не прошла: {e}");
        eprintln!(
            "новый пароль в sec ({key}); восстановить старый — sec undo {key} (и повторно ALTER)"
        );
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
