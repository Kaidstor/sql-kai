//! `kai holder` — фоновый держатель cli-сессий на то время, когда GUI не
//! запущен. Первый `kai q` спавнит его сам (см. broker_client::connect_any),
//! дальше он обслуживает вызовы kai тем же broker-протоколом, что и GUI:
//! pg-сессии и ssh-туннели живут между запусками (запросы не платят за
//! connect), транзакция из одного вызова доживает до следующего.
//!
//! Жизненный цикл: vault разлочивается только тихой цепочкой (trust/env) —
//! не смогли, значит holder не поднимается и kai работает автономно.
//! Самозавершается, когда сессий не осталось и сокет простаивает; vault lock
//! в GUI (и `kai holder stop`) гасит его немедленно.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Subcommand;
use sql_kai_lib::broker::{self, BrokerHooks, BrokerState};
use sql_kai_lib::error::AppError;
use sql_kai_lib::logging;

use crate::broker_client;
use crate::session;

/// Без сессий и запросов по сокету дольше этого — holder выходит.
/// Сессии сами живут по TTL брокера (15 мин / 180 с в транзакции), так что
/// это только «хвост» после того, как последняя закрылась.
const LINGER_SEC: u64 = 120;

#[derive(Subcommand)]
pub enum HolderCmd {
    /// Запустить holder в текущем процессе (обычно спавнится сам)
    Run,
    /// Погасить запущенный holder (закрыть его сессии)
    Stop,
}

pub async fn run(cmd: HolderCmd) -> Result<ExitCode, AppError> {
    match cmd {
        HolderCmd::Run => serve().await,
        HolderCmd::Stop => stop().await,
    }
}

fn remove_socket() {
    if let Ok(p) = broker::holder_socket_path() {
        let _ = std::fs::remove_file(&p);
    }
}

async fn serve() -> Result<ExitCode, AppError> {
    if let Err(e) = session::unlock_vault_headless() {
        logging::log("holder", &format!("не поднялся: {e}"));
        return Err(e);
    }
    // Мастер-пароль из env сделал своё дело — не держать его в окружении
    // долгоживущего процесса и не отдавать детям (ssh-туннелям).
    std::env::remove_var("KAI_VAULT_PASSWORD");
    let Some(listener) = broker::bind_holder().await? else {
        return Ok(ExitCode::SUCCESS); // живой holder уже есть
    };

    let state = Arc::new(BrokerState::default());
    state.touch(); // точка отсчёта простоя — старт
    let shutdown_state = state.clone();
    let hooks = Arc::new(BrokerHooks {
        gui_sessions: Box::new(Vec::new),
        changed: Box::new(|| {}),
        profiles_changed: Box::new(|| {}),
        shutdown: Some(Box::new(move || {
            shutdown_state.clear();
            logging::log("holder", "погашен по shutdown (vault lock / holder stop)");
            // ответ клиенту уходит после хука — выходим с небольшой паузой
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(150)).await;
                remove_socket();
                std::process::exit(0);
            });
        })),
    });

    // Самозавершение: активность отмечает каждый запрос (BrokerState::touch в
    // dispatch), так что открывающийся именно сейчас клиент не попадёт под нож.
    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                if state.is_idle(LINGER_SEC) {
                    logging::log("holder", "сессий нет, сокет простаивает — выходим");
                    remove_socket();
                    std::process::exit(0);
                }
            }
        });
    }

    logging::log(
        "holder",
        &format!("запущен (pid {}, v{})", std::process::id(), env!("CARGO_PKG_VERSION")),
    );
    broker::serve(listener, state, hooks).await;
    Ok(ExitCode::SUCCESS)
}

async fn stop() -> Result<ExitCode, AppError> {
    if broker_client::connect_holder().await.is_none() {
        println!("holder не запущен");
        return Ok(ExitCode::SUCCESS);
    }
    broker::shutdown_holder().await;
    println!("holder погашен");
    Ok(ExitCode::SUCCESS)
}
