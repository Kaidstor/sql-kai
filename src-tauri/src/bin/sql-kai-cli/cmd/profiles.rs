//! `sql-kai profiles` — профили подключений (общие с GUI): список, просмотр,
//! удаление (вместе с секретами из vault).

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
        /// Фильтр: подстрока без учёта регистра по name/group/host/db/ssh
        /// (обычно имя сервиса: `sql-kai profiles list vuln`)
        filter: Option<String>,
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

impl ProfilesCmd {
    /// Подкоманда по умолчанию: голый `sql-kai profiles` = `profiles list`.
    pub fn default_list() -> Self {
        Self::List {
            filter: None,
            fmt: Default::default(),
        }
    }
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
        ProfilesCmd::List { filter, fmt } => {
            let mut profiles = store::load_profiles()?;
            if let Some(f) = &filter {
                let f = f.to_lowercase();
                profiles.retain(|p| {
                    p.name.to_lowercase().contains(&f)
                        || p.group
                            .as_deref()
                            .is_some_and(|g| g.to_lowercase().contains(&f))
                        || p.database.to_lowercase().contains(&f)
                        || format!("{}:{}", p.host, p.port).contains(&f)
                        || p.ssh
                            .as_ref()
                            .is_some_and(|s| s.host.to_lowercase().contains(&f))
                });
            }
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
                let q = format!(
                    "удалить профиль '{}' ({}:{}/{})?",
                    p.name, p.host, p.port, p.database
                );
                if !crate::input::confirm(&q, "--yes")? {
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
