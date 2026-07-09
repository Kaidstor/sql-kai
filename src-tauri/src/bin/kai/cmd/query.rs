//! `kai q <alias>` — выполнить SQL в базе профиля (`kai <alias>` — то же
//! самое). Живой GUI обслуживает запрос через брокер, иначе — автономная
//! сессия.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use sql_kai_lib::db;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, HistoryEntry};

use crate::output::{self, Format};
use crate::{broker_client, redact, session};

#[derive(Args)]
pub struct FormatArgs {
    /// JSON-вывод (columns/rows/rowsAffected/truncated); значения приведены
    /// к типам колонок БД: числа/bool/null/json (точное строковое
    /// представление — кастуй в ::text)
    #[arg(long, group = "fmt")]
    json: bool,
    /// CSV-вывод
    #[arg(long, group = "fmt")]
    csv: bool,
    /// tuples-only: значения через |, без заголовков
    #[arg(short = 't', long, group = "fmt")]
    tuples: bool,
}

impl FormatArgs {
    fn pick(&self) -> Format {
        if self.json {
            Format::Json
        } else if self.csv {
            Format::Csv
        } else if self.tuples {
            Format::Tuples
        } else {
            Format::Table
        }
    }
}

#[derive(Args)]
pub struct QueryArgs {
    /// Профиль: имя, id или группа
    pub(crate) alias: String,
    /// SQL-команда (можно повторять)
    #[arg(short = 'c', long = "command", value_name = "SQL")]
    pub(crate) commands: Vec<String>,
    /// Файл с SQL (можно повторять)
    #[arg(short = 'f', long = "file", value_name = "FILE")]
    pub(crate) files: Vec<PathBuf>,
    #[command(flatten)]
    pub(crate) fmt: FormatArgs,
    /// Максимум строк на результат
    #[arg(long, default_value_t = 1000, value_name = "N")]
    pub(crate) max_rows: usize,
    /// Разрешить запись (по умолчанию сессия read-only)
    #[arg(long)]
    pub(crate) write: bool,
    /// Env-переменная с паролем БД (обход vault)
    #[arg(long, value_name = "VAR")]
    pub(crate) password_env: Option<String>,
    /// Взять пароль БД из sec (ключ <имя>/DB_PASSWORD)
    #[arg(long)]
    pub(crate) from_sec: bool,
    /// Ключ sec для пароля (proj/KEY); включает --from-sec
    #[arg(long, value_name = "PROJ/KEY")]
    pub(crate) sec_key: Option<String>,
    /// Не записывать запрос в историю
    #[arg(long)]
    pub(crate) no_history: bool,
    /// Не маскировать чувствительные колонки (password/secret/*_token/*_key)
    #[arg(long)]
    pub(crate) no_redact: bool,
    /// Не переиспользовать ssh-туннель (без ControlMaster)
    #[arg(long)]
    pub(crate) no_mux: bool,
    /// Не ходить через брокер запущенного GUI — всегда своя сессия
    #[arg(long)]
    pub(crate) local: bool,
    /// Показать, куда подключились
    #[arg(short, long)]
    pub(crate) verbose: bool,
}

/// Общий рендер результата: маскирование, формат (+типизированный json),
/// verbose-времена. `types` пустой, когда формат не json.
fn render_exec(a: &QueryArgs, mut exec: db::ExecResult, types: &[Option<Vec<(String, db::Type)>>]) {
    if !a.no_redact {
        let masked = redact::redact_exec(&mut exec);
        if !masked.is_empty() {
            eprintln!(
                "⚠ kai: маскированы чувствительные колонки: {} (показать: --no-redact)",
                masked.join(", ")
            );
        }
    }
    let fmt = a.fmt.pick();
    if fmt == Format::Json {
        let untyped = output::print_exec_json(&exec, types);
        if untyped > 0 {
            eprintln!(
                "⚠ kai: не удалось определить типы колонок для {untyped} стейтмент(а/ов) — их значения строками"
            );
        }
    } else {
        output::print_exec(&exec, fmt);
    }
    if a.verbose {
        eprintln!("({} ms)", exec.duration_ms);
    }
}

/// (имя, oid) с провода → типы для print_exec_json; незнакомый oid (кастомный
/// enum и т.п.) выводится как text — так же он выглядит и в автономном пути.
fn wire_types(
    wire: sql_kai_lib::broker::WireColumnTypes,
) -> Vec<Option<Vec<(String, db::Type)>>> {
    wire.into_iter()
        .map(|cols| {
            cols.map(|cols| {
                cols.into_iter()
                    .map(|(name, oid)| (name, db::Type::from_oid(oid).unwrap_or(db::Type::TEXT)))
                    .collect()
            })
        })
        .collect()
}

/// Пытается обслужить запрос брокером запущенного GUI. Some(exit) — запрос
/// выполнен (или окончательно отвергнут) брокером; None — GUI недоступен или
/// vault заперт, и нужно идти автономным путём.
async fn try_broker_query(a: &QueryArgs, sql: &str) -> Result<Option<ExitCode>, AppError> {
    let Some(mut b) = broker_client::connect().await else {
        return Ok(None);
    };
    let profile = session::resolve_profile(&a.alias)?;
    if a.verbose {
        eprintln!("→ через брокер GUI {} (сессию держит приложение)", b.hello.server_version);
    }
    let with_types = a.fmt.pick() == Format::Json;
    let outcome = tokio::select! {
        r = b.query(&profile.id, sql, a.max_rows.max(1), a.write, with_types) => r,
        _ = tokio::signal::ctrl_c() => {
            // наш сокет занят ожиданием ответа — отмену шлём новым соединением
            if let Some(mut c) = broker_client::connect().await {
                let _ = c.cancel(&profile.id).await;
            }
            eprintln!("kai: отменено");
            return Ok(Some(ExitCode::FAILURE));
        }
    };
    let record = |ok: bool| {
        if !a.no_history {
            let _ = store::record_history(HistoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                profile_id: profile.id.clone(),
                profile_name: profile.name.clone(),
                sql: sql.to_string(),
                at: session::now_ms(),
                ok,
            });
        }
    };
    match outcome {
        Ok(res) => {
            record(true);
            let types = res.column_types.map(wire_types).unwrap_or_default();
            render_exec(a, res.exec, &types);
            Ok(Some(ExitCode::SUCCESS))
        }
        Err(broker_client::BrokerError::Query { message, sqlstate }) => {
            record(false);
            eprintln!("kai: {message}");
            // 25006 read_only_sql_transaction — код, а не поиск по тексту
            if sqlstate.as_deref() == Some("25006") {
                eprintln!("hint: сессия read-only по умолчанию — повтори с --write, если изменение согласовано");
            }
            Ok(Some(ExitCode::FAILURE))
        }
        Err(broker_client::BrokerError::VaultLocked) => {
            // Брокер отверг запрос ДО выполнения — автономный путь безопасен.
            if a.verbose {
                eprintln!("… vault в GUI заперт — автономный режим");
            }
            Ok(None)
        }
        Err(e) => {
            // Транспорт мог оборваться уже ПОСЛЕ отправки запроса (GUI закрыли
            // во время выполнения) — стейтмент мог успеть примениться. Повторять
            // write-SQL автономным путём нельзя: риск двойного применения.
            if a.write {
                record(false);
                eprintln!("kai: связь с брокером оборвалась во время запроса ({e})");
                eprintln!(
                    "hint: запрос мог успеть выполниться — проверь состояние данных; \
                     выполнить мимо брокера: kai q --local …"
                );
                return Ok(Some(ExitCode::FAILURE));
            }
            if a.verbose {
                eprintln!("… брокер недоступен ({e}) — автономный режим");
            }
            Ok(None)
        }
    }
}

pub async fn run(a: QueryArgs) -> Result<ExitCode, AppError> {
    let sql = session::collect_sql(&a.commands, &a.files)?;
    // Живой GUI обслуживает запрос своей cli-сессией; кастомные источники
    // пароля (--password-env/--from-sec) — всегда автономно.
    if !a.local && a.password_env.is_none() && !a.from_sec && a.sec_key.is_none() {
        if let Some(code) = try_broker_query(&a, &sql).await? {
            return Ok(code);
        }
    }
    let (profile, connected) = session::open_for(
        &a.alias,
        session::PwSource {
            env: a.password_env.as_deref(),
            from_sec: a.from_sec,
            sec_key: a.sec_key.as_deref(),
        },
        a.write,
        a.verbose,
        !a.no_mux,
    )
    .await?;

    let outcome = db::execute(&connected.session.client, &sql, a.max_rows.max(1)).await;
    if !a.no_history {
        let _ = store::record_history(HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            sql: sql.clone(),
            at: session::now_ms(),
            ok: outcome.is_ok(),
        });
    }
    match outcome {
        Ok(exec) => {
            let types = if a.fmt.pick() == Format::Json {
                db::statement_column_types(&connected.session.client, &sql).await
            } else {
                Vec::new()
            };
            render_exec(&a, exec, &types);
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            eprintln!("kai: {e}");
            if e.is_read_only() {
                eprintln!("hint: сессия read-only по умолчанию — повтори с --write, если изменение согласовано");
            }
            Ok(ExitCode::FAILURE)
        }
    }
}
