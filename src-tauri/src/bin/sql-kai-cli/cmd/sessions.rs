//! `sql-kai sessions` — живые сессии обоих серверов: GUI (его собственные и
//! cli-сессии брокера) и holder'а (origin=holder).

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

/// Живые сессии GUI (origin=gui — собственные, origin=cli — брокерские) и
/// holder'а (origin=holder); у cli/holder показан простой.
pub async fn run(a: SessionsArgs) -> Result<ExitCode, AppError> {
    let gui = broker_client::connect().await;
    let holder = broker_client::connect_holder().await;
    if gui.is_none() && holder.is_none() {
        eprintln!("ни GUI, ни holder не запущены — sql-kai работает автономно");
        return Ok(ExitCode::FAILURE);
    }
    let mut sources = Vec::new();
    let mut list = Vec::new();
    if let Some(mut b) = gui {
        list.extend(
            b.sessions()
                .await
                .map_err(|e| AppError::Msg(e.to_string()))?,
        );
        sources.push(format!("gui {}", b.hello.server_version));
    }
    if let Some(mut b) = holder {
        let mut hs = b
            .sessions()
            .await
            .map_err(|e| AppError::Msg(e.to_string()))?;
        // сервер шлёт origin=cli — помечаем, что держит их фоновый holder
        for s in &mut hs {
            s.origin = "holder".into();
        }
        list.extend(hs);
        sources.push(format!("holder {}", b.hello.server_version));
    }
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
        println!("живых сессий нет ({})", sources.join(", "));
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
