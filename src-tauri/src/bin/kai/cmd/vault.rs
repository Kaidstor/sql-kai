//! `kai vault` — vault с паролями БД: статус, создание, тихий доступ CLI
//! (trust через keychain) и его отзыв.

use std::process::ExitCode;

use clap::Subcommand;
use sql_kai_lib::error::AppError;
use sql_kai_lib::vault;

use crate::session;

#[derive(Subcommand)]
pub enum VaultCmd {
    /// Есть ли vault, Touch ID, CLI-trust
    Status,
    /// Создать vault (мастер-пароль)
    Setup,
    /// Разрешить CLI тихий доступ к vault через keychain
    Trust,
    /// Отозвать CLI-доступ
    Revoke,
}

pub fn run(cmd: VaultCmd) -> Result<ExitCode, AppError> {
    match cmd {
        VaultCmd::Status => {
            println!("vault:     {}", if vault::exists() { "есть" } else { "нет" });
            println!(
                "touch id:  {}",
                if vault::biometric_enrolled() { "включён" } else { "выключен" }
            );
            println!(
                "cli trust: {}",
                if vault::cli_trust_enrolled() { "включён" } else { "выключен" }
            );
        }
        VaultCmd::Setup => {
            if vault::exists() {
                return Err(AppError::Msg("vault уже существует".into()));
            }
            let pw = session::read_new_password()?;
            vault::setup(&pw)?;
            println!("vault создан и разблокирован; пароли профилей будут храниться в нём");
        }
        VaultCmd::Trust => {
            session::unlock_vault()?;
            vault::enable_cli_trust()?;
            println!(
                "cli trust включён: kai будет читать пароли без запроса \
                 (копия ключа в login keychain; отозвать — `kai vault revoke`)"
            );
        }
        VaultCmd::Revoke => {
            vault::disable_cli_trust();
            println!("cli trust отозван");
        }
    }
    Ok(ExitCode::SUCCESS)
}
