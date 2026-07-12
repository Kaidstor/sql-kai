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

/// Компактное «сколько назад» для колонки last: 45s / 12m / 5h / 3d / 2mo / 1y.
fn fmt_ago(ts_ms: i64) -> String {
    let s = ((store::now_ms() - ts_ms) / 1000).max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3_599 => format!("{}m", s / 60),
        3_600..=86_399 => format!("{}h", s / 3_600),
        86_400..=2_591_999 => format!("{}d", s / 86_400),
        2_592_000..=31_535_999 => format!("{}mo", s / 2_592_000),
        _ => format!("{}y", s / 31_536_000),
    }
}

pub async fn run(cmd: ProfilesCmd) -> Result<ExitCode, AppError> {
    match cmd {
        ProfilesCmd::List { fmt } => {
            let mut profiles = store::load_profiles()?;
            let marks = store::load_last_connected().unwrap_or_default();
            for p in &mut profiles {
                p.last_connected = marks.get(&p.id).copied();
            }
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
                        p.last_connected
                            .as_ref()
                            .and_then(|lc| lc.latest())
                            .map(|(ts, via)| format!("{} ({via})", fmt_ago(ts))),
                    ]
                })
                .collect();
            output::print_rows(
                &["name", "group", "host", "db", "user", "ssh", "pw", "last"],
                &rows,
                fmt.pick(),
            );
        }
        ProfilesCmd::Show { alias } => {
            let mut p = session::resolve_profile(&alias)?;
            p.last_connected = store::load_last_connected()
                .unwrap_or_default()
                .remove(&p.id);
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
            crate::broker_client::notify_profiles_changed().await;
        }
    }
    Ok(ExitCode::SUCCESS)
}
