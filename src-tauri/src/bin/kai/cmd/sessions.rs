//! `kai sessions` — живые сессии запущенного GUI: его собственные и
//! cli-сессии брокера.

use std::process::ExitCode;

use clap::Args;
use sql_kai_lib::error::AppError;

use crate::broker_client;

#[derive(Args)]
pub struct SessionsArgs {
    /// JSON-вывод
    #[arg(long)]
    json: bool,
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
    if a.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".into())
        );
        return Ok(ExitCode::SUCCESS);
    }
    if list.is_empty() {
        println!("живых сессий нет (gui {})", b.hello.server_version);
        return Ok(ExitCode::SUCCESS);
    }
    println!("{:<7} {:<28} {:<7} {:<8} {}", "ORIGIN", "PROFILE", "TX", "TUNNEL", "IDLE");
    for s in list {
        println!(
            "{:<7} {:<28} {:<7} {:<8} {}",
            s.origin,
            s.profile_name,
            s.tx,
            s.tunnel_port
                .map(|p| format!(":{p}"))
                .unwrap_or_else(|| "-".into()),
            s.idle_sec
                .map(|i| format!("{i}s"))
                .unwrap_or_else(|| "-".into()),
        );
    }
    Ok(ExitCode::SUCCESS)
}
