//! `kai sessions` — живые сессии запущенного GUI: его собственные и
//! cli-сессии брокера.

use std::process::ExitCode;

use clap::Args;
use sql_kai_lib::error::AppError;

use crate::broker_client;
use crate::output::{self, Format, FormatArgs};

#[derive(Args)]
pub struct SessionsArgs {
    #[command(flatten)]
    fmt: FormatArgs,
}

/// Живые сессии запущенного GUI: собственные (origin=gui) и cli-сессии
/// брокера (origin=cli, с простоем).
pub async fn run(a: SessionsArgs) -> Result<ExitCode, AppError> {
    let Some(mut b) = broker_client::connect().await else {
        eprintln!("GUI не запущен — брокер недоступен (kai работает автономно)");
        return Ok(ExitCode::FAILURE);
    };
    let list = b
        .sessions()
        .await
        .map_err(|e| AppError::Msg(e.to_string()))?;
    let fmt = a.fmt.pick();
    // --json отдаёт полные объекты (profileId и т.п.), не табличную проекцию
    if fmt == Format::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".into())
        );
        return Ok(ExitCode::SUCCESS);
    }
    if list.is_empty() && fmt == Format::Table {
        println!("живых сессий нет (gui {})", b.hello.server_version);
        return Ok(ExitCode::SUCCESS);
    }
    let rows: Vec<Vec<Option<String>>> = list
        .into_iter()
        .map(|s| {
            vec![
                Some(s.origin),
                Some(s.profile_name),
                Some(s.tx),
                Some(
                    s.tunnel_port
                        .map(|p| format!(":{p}"))
                        .unwrap_or_else(|| "-".into()),
                ),
                Some(
                    s.idle_sec
                        .map(|i| format!("{i}s"))
                        .unwrap_or_else(|| "-".into()),
                ),
            ]
        })
        .collect();
    output::print_rows(&["origin", "profile", "tx", "tunnel", "idle"], &rows, fmt);
    Ok(ExitCode::SUCCESS)
}
