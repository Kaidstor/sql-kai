//! `kai profiles` — профили подключений (общие с GUI): список, просмотр,
//! удаление (вместе с секретами из vault).

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Subcommand;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store;

use crate::output::{self, Format, FormatArgs};
use crate::session;

#[derive(Subcommand)]
pub enum ProfilesCmd {
    /// Список профилей
    List {
        #[command(flatten)]
        fmt: FormatArgs,
    },
    /// Показать профиль
    Show { alias: String },
    /// Удалить профиль (и его секреты из vault)
    Rm {
        alias: String,
        /// Не спрашивать подтверждение
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(cmd: ProfilesCmd) -> Result<ExitCode, AppError> {
    match cmd {
        ProfilesCmd::List { fmt } => {
            let profiles = store::load_profiles()?;
            // --json отдаёт полные объекты профилей, не табличную проекцию
            if fmt.pick() == Format::Json {
                println!("{}", serde_json::to_string_pretty(&profiles).unwrap());
                return Ok(ExitCode::SUCCESS);
            }
            let rows: Vec<Vec<Option<String>>> = profiles
                .iter()
                .map(|p| {
                    vec![
                        Some(p.name.clone()),
                        p.group.clone(),
                        Some(format!("{}:{}", p.host, p.port)),
                        Some(p.database.clone()),
                        Some(p.user.clone()),
                        p.ssh.as_ref().map(|s| s.host.clone()),
                        Some(if p.has_password { "yes" } else { "no" }.into()),
                    ]
                })
                .collect();
            output::print_rows(
                &["name", "group", "host", "db", "user", "ssh", "pw"],
                &rows,
                fmt.pick(),
            );
        }
        ProfilesCmd::Show { alias } => {
            let p = session::resolve_profile(&alias)?;
            println!("{}", serde_json::to_string_pretty(&p).unwrap());
        }
        ProfilesCmd::Rm { alias, yes } => {
            let p = session::resolve_profile(&alias)?;
            if !yes {
                if !std::io::stdin().is_terminal() {
                    return Err(AppError::Msg(
                        "нет TTY для подтверждения — добавь --yes".into(),
                    ));
                }
                eprint!("удалить профиль '{}' ({}:{}/{})? [y/N] ", p.name, p.host, p.port, p.database);
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if !matches!(line.trim(), "y" | "Y" | "yes") {
                    eprintln!("отменено");
                    return Ok(ExitCode::FAILURE);
                }
            }
            store::delete_profile(&p.id)?;
            println!("профиль '{}' удалён", p.name);
        }
    }
    Ok(ExitCode::SUCCESS)
}
