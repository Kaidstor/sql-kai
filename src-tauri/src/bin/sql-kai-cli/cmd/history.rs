//! `sql-kai history` — история выполненных запросов (общая с GUI) и её проверка
//! на утёкшие секреты (`--scan` через `sec scan`).

use std::process::ExitCode;

use clap::Args;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store;

use crate::output::{self, Format, FormatArgs};
use crate::{envelope, sec, session};

#[derive(Args)]
pub struct HistoryArgs {
    /// Фильтр по профилю (имя или id)
    alias: Option<String>,
    /// Сколько записей показать
    #[arg(short = 'n', long, default_value_t = 20)]
    limit: usize,
    #[command(flatten)]
    fmt: FormatArgs,
    /// Прогнать history.json через `sec scan` — не утёк ли секрет в открытом виде
    /// (из форматов — только --json: отчёт sec текстовый, таблицы у него нет)
    #[arg(long, conflicts_with_all = ["csv", "tuples"])]
    scan: bool,
}

pub fn run(a: HistoryArgs) -> Result<ExitCode, AppError> {
    if a.scan {
        return history_scan(a.fmt.pick() == Format::Json);
    }
    let mut entries = store::load_history()?;
    if let Some(alias) = &a.alias {
        // Как в `sql-kai q`: alias матчит id, имя и группу; плюс имя из самой
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
    // --json отдаёт полные записи (id/at/ok), не табличную проекцию
    if a.fmt.pick() == Format::Json {
        envelope::print_data(&entries);
        return Ok(ExitCode::SUCCESS);
    }
    let now = store::now_ms();
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
    output::print_rows(&["when", "profile", "st", "sql"], &rows, a.fmt.pick());
    Ok(ExitCode::SUCCESS)
}

/// Прогоняет history.json через `sec scan` — не осел ли где секрет в открытом
/// виде (sql-kai редактирует пароли при записи, но старые записи или неожиданные
/// литералы мог поймать sec).
///
/// `--json`: чисто — конверт успеха с `{file, found: false, report}` в `data`;
/// найдено — конверт отказа `kind: secrets_found`, код 1, те же поля в `data`. `report` — текст sec как есть: своего JSON у
/// `sec scan` нет.
fn history_scan(json: bool) -> Result<ExitCode, AppError> {
    sec::available()?;
    let path = sql_kai_lib::fsio::config_path("history.json")?;
    let file = path.to_string_lossy().into_owned();
    let (found, report) = if path.exists() {
        sec::scan(&file)?
    } else {
        (false, String::new())
    };
    let data = serde_json::json!({ "file": file, "found": found, "report": report });
    if found {
        let message = "в истории найдены значения секретов из sec — почисти: `sec forget <ключ>` \
                       и удали затронутые записи в history.json";
        if json {
            return Ok(envelope::fail_with_data(
                "secrets_found",
                message,
                &[],
                data,
            ));
        }
        println!("{report}");
        println!("⚠ {message}");
        return Ok(ExitCode::FAILURE);
    }
    if json {
        envelope::print_success(data, &[]);
    } else if path.exists() {
        println!("чисто: секретов sec в history.json не найдено");
    } else {
        println!("history.json пуст — сканировать нечего");
    }
    Ok(ExitCode::SUCCESS)
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
