//! `kai history` — история выполненных запросов (общая с GUI) и её проверка
//! на утёкшие секреты (`--scan` через `sec scan`).

use std::process::ExitCode;

use clap::Args;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store;

use crate::{output, sec, session};

#[derive(Args)]
pub struct HistoryArgs {
    /// Фильтр по профилю (имя или id)
    alias: Option<String>,
    /// Сколько записей показать
    #[arg(short = 'n', long, default_value_t = 20)]
    limit: usize,
    #[arg(long)]
    json: bool,
    /// Прогнать history.json через `sec scan` — не утёк ли секрет в открытом виде
    #[arg(long)]
    scan: bool,
}

pub fn run(a: HistoryArgs) -> Result<ExitCode, AppError> {
    if a.scan {
        return history_scan();
    }
    let mut entries = store::load_history()?;
    if let Some(alias) = &a.alias {
        // Как в `kai q`: alias матчит id, имя и группу; плюс имя из самой
        // записи — чтобы история удалённых профилей оставалась доступной.
        let ids: std::collections::HashSet<String> =
            session::filter_profiles(&store::load_profiles().unwrap_or_default(), alias)
                .into_iter()
                .map(|p| p.id)
                .collect();
        let al = alias.to_lowercase();
        entries.retain(|h| {
            ids.contains(&h.profile_id)
                || h.profile_name.to_lowercase() == al
                || h.profile_id == *alias
        });
    }
    entries.truncate(a.limit);
    if a.json {
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
        return Ok(ExitCode::SUCCESS);
    }
    let now = session::now_ms();
    let rows: Vec<Vec<Option<String>>> = entries
        .iter()
        .map(|h| {
            vec![
                Some(age(now - h.at)),
                Some(h.profile_name.clone()),
                Some(if h.ok { "ok" } else { "err" }.into()),
                Some(one_line(&h.sql, 100)),
            ]
        })
        .collect();
    output::print_rows(&["when", "profile", "st", "sql"], &rows, false);
    Ok(ExitCode::SUCCESS)
}

/// Прогоняет history.json через `sec scan` — не осел ли где секрет в открытом
/// виде (kai редактирует пароли при записи, но старые записи или неожиданные
/// литералы мог поймать sec).
fn history_scan() -> Result<ExitCode, AppError> {
    sec::available()?;
    let path = sql_kai_lib::fsio::config_path("history.json")?;
    if !path.exists() {
        println!("history.json пуст — сканировать нечего");
        return Ok(ExitCode::SUCCESS);
    }
    let (found, report) = sec::scan(&path.to_string_lossy())?;
    if found {
        println!("{report}");
        println!("⚠ в истории найдены значения секретов из sec — почисти: `sec forget <ключ>` и удали затронутые записи в history.json");
        Ok(ExitCode::FAILURE)
    } else {
        println!("чисто: секретов sec в history.json не найдено");
        Ok(ExitCode::SUCCESS)
    }
}

fn age(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

pub(crate) fn one_line(sql: &str, max: usize) -> String {
    let flat = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}
