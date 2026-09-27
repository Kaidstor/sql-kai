//! `sql-kai q <alias>` — выполнить SQL в базе профиля (`sql-kai <alias>` — то же
//! самое). Живой GUI обслуживает запрос через брокер, иначе — автономная
//! сессия.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use sql_kai_lib::db;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, HistoryEntry};

use crate::output::{self, Format, FormatArgs};
use crate::{broker_client, envelope, input, redact, session};

const READ_ONLY_HINT: &str =
    "hint: сессия read-only по умолчанию — повтори с --write, если изменение согласовано";

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
    /// Подтвердить запись в production-профиль (только вместе с --write)
    #[arg(long, requires = "write")]
    pub(crate) prod_write: bool,
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
    /// Не ходить через GUI/holder — всегда своя одноразовая сессия
    #[arg(long)]
    pub(crate) local: bool,
    /// Показать, куда подключились
    #[arg(short, long)]
    pub(crate) verbose: bool,
}

/// Notices сервера и предупреждение об откате — в stderr, как у psql. Идут до
/// результата: сервер присылает их раньше, чем строки.
fn report_server_messages(
    notices: &[db::Notice],
    dropped: u64,
    left: Option<db::TxLeftover>,
    error: Option<&str>,
) {
    for line in db::server_message_lines(notices, dropped, 0, left, error) {
        eprintln!("{line}");
    }
}

/// Общий рендер результата: маскирование, формат (+типизированный json),
/// verbose-времена. `types` пустой, когда формат не json.
fn render_exec(a: &QueryArgs, mut exec: db::ExecResult, types: &[Option<Vec<(String, db::Type)>>]) {
    report_server_messages(&exec.notices, exec.notices_dropped, None, None);
    let fmt = a.fmt.pick();
    // под --json предупреждения едут полем warning конверта, иначе — в stderr
    let mut warnings = Vec::new();
    if !a.no_redact {
        let masked = redact::redact_exec(&mut exec);
        if !masked.is_empty() {
            warnings.push(format!(
                "маскированы чувствительные колонки: {} (показать: --no-redact)",
                masked.join(", ")
            ));
        }
    }
    if fmt == Format::Json {
        let (data, untyped) = output::exec_json(&exec, types);
        if untyped > 0 {
            warnings.push(format!(
                "не удалось определить типы колонок для {untyped} стейтмент(а/ов) — их значения строками"
            ));
        }
        envelope::print_success(data, &warnings);
    } else {
        for w in &warnings {
            eprintln!("⚠ sql-kai: {w}");
        }
        output::print_exec(&exec, fmt);
    }
    if a.verbose {
        eprintln!("({} ms)", exec.duration_ms);
    }
}

/// (имя, oid) с провода → типы для exec_json; незнакомый oid (кастомный
/// enum и т.п.) выводится как text — так же он выглядит и в автономном пути.
fn wire_types(wire: sql_kai_lib::broker::WireColumnTypes) -> Vec<Option<Vec<(String, db::Type)>>> {
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

/// Пытается обслужить запрос сервером сессий — брокером запущенного GUI или
/// holder'ом (спавнится по требованию). Some(exit) — запрос выполнен (или
/// окончательно отвергнут) сервером; None — сервера нет или vault заперт,
/// и нужно идти автономным путём.
async fn try_broker_query(a: &QueryArgs, sql: &str) -> Result<Option<ExitCode>, AppError> {
    let Some(mut b) = broker_client::connect_any().await else {
        return Ok(None);
    };
    let profile = session::resolve_profile(&a.alias)?;
    if a.verbose {
        match b.via {
            broker_client::Via::Gui => eprintln!(
                "→ через брокер GUI {} (сессию держит приложение)",
                b.hello.server_version
            ),
            broker_client::Via::Holder => eprintln!(
                "→ через holder {} (фоновый держатель сессий)",
                b.hello.server_version
            ),
        }
    }
    let with_types = a.fmt.pick() == Format::Json;
    let outcome = tokio::select! {
        r = b.query(&profile.id, sql, a.max_rows.max(1), a.write, with_types) => r,
        _ = tokio::signal::ctrl_c() => {
            // наш сокет занят ожиданием ответа — отмену шлём новым соединением
            if let Some(mut c) = broker_client::reconnect(b.via).await {
                let _ = c.cancel(&profile.id).await;
            }
            return Ok(Some(envelope::fail("cancelled", "отменено", &[])));
        }
    };
    let record = |ok: bool| {
        if !a.no_history {
            let _ = store::record_history(HistoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                profile_id: profile.id.clone(),
                profile_name: profile.name.clone(),
                sql: sql.to_string(),
                at: store::now_ms(),
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
        Err(broker_client::BrokerError::Query {
            message,
            sqlstate,
            notices,
            notices_dropped,
            tx_rolled_back,
        }) => {
            record(false);
            report_server_messages(&notices, notices_dropped, tx_rolled_back, Some(&message));
            let mut hints = Vec::new();
            // 25006 read_only_sql_transaction — код, а не поиск по тексту
            if sqlstate.as_deref() == Some("25006") {
                hints.push(READ_ONLY_HINT.to_string());
            }
            // Сессию сервера оставил в aborted прошлый батч, а сервер старый и
            // сам её не чистит. ROLLBACK без --write проходит и на нём.
            if sqlstate.as_deref() == Some("25P02")
                || message.contains("current transaction is aborted")
            {
                hints.push(format!(
                    "hint: сессия сервера осталась в прерванной транзакции (старая версия GUI/holder) — \
                     `sql-kai {} -c ROLLBACK` без --write, либо `sql-kai holder stop`, либо --local",
                    a.alias
                ));
            }
            let kind = match sqlstate.as_deref() {
                Some("25006") => "read_only",
                Some(_) => "db",
                None => "refused",
            };
            Ok(Some(envelope::fail(kind, &message, &hints)))
        }
        Err(broker_client::BrokerError::ProdGuard(message)) => {
            record(false);
            Ok(Some(envelope::fail("prod_guard", &message, &[])))
        }
        Err(broker_client::BrokerError::VaultLocked) => {
            // Брокер отверг запрос ДО выполнения — автономный путь безопасен.
            if a.verbose {
                eprintln!("… vault в GUI заперт — автономный режим");
            }
            Ok(None)
        }
        Err(broker_client::BrokerError::Connect(message)) => {
            // Сервер не открыл сессию: SQL до базы не дошёл. Поэтому и на
            // --write уходим автономным путём — двойного применения тут быть не
            // может, а человек увидит настоящую ошибку подключения (обычно та же
            // самая: база не поднята) вместо разговора про оборванный брокер.
            // Прод-барьер пройден выше в run(), до этой развилки.
            if a.verbose {
                eprintln!("… брокер не подключился к базе ({message}) — автономный режим");
            }
            Ok(None)
        }
        Err(e) => {
            // Остались настоящие транспортные обрывы: связь могла умереть уже
            // ПОСЛЕ отправки запроса (GUI закрыли во время выполнения) — тогда
            // стейтмент успел примениться. Повторять write-SQL автономным путём
            // нельзя: риск двойного применения.
            if a.write {
                record(false);
                return Ok(Some(envelope::fail(
                    "connection_lost",
                    &format!("связь с брокером оборвалась во время запроса ({e})"),
                    &[
                        "hint: запрос мог успеть выполниться — проверь состояние данных; \
                       выполнить мимо брокера: sql-kai q --local …"
                            .to_string(),
                    ],
                )));
            }
            if a.verbose {
                eprintln!("… брокер недоступен ({e}) — автономный режим");
            }
            Ok(None)
        }
    }
}

pub async fn run(a: QueryArgs) -> Result<ExitCode, AppError> {
    let sql = input::collect_sql(&a.commands, &a.files)?;
    // Прод-барьер — до выбора пути: подтверждение спрашивается один раз и до
    // того, как поднимутся брокер/holder/туннель. Дальше и брокерный, и
    // автономный путь видят уже выданное разрешение.
    if a.write {
        let profile = session::resolve_profile(&a.alias)?;
        session::authorize_prod_write(&profile, a.prod_write)?;
    }
    // Живой GUI (или holder) обслуживает запрос своей cli-сессией; кастомные
    // источники пароля (--password-env/--from-sec) — всегда автономно.
    // --no-mux тоже: сервер сессий держит mux-туннели, а флаг просит свежий
    // ssh без ControlMaster.
    if !a.local && !a.no_mux && a.password_env.is_none() && !a.from_sec && a.sec_key.is_none() {
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

    // Автономная сессия (--local/--no-mux) идёт мимо брокера, поэтому read-only
    // тут обеспечивает та же обёртка BEGIN READ ONLY, что и там: одного
    // default_transaction_read_only мало — батч снимает его сам.
    let client = &connected.session.client;
    let outcome = if a.write {
        db::execute(client, &sql, a.max_rows.max(1)).await
    } else {
        db::execute_read_only(client, &sql, a.max_rows.max(1)).await
    };
    let notices = connected.session.notices.take();
    // Сессия одноразовая, и оставленную транзакцию сервер откатил бы сам при
    // закрытии соединения — но молча: «UPDATE 5» без COMMIT выглядел бы
    // применённым. Проверяем явно, чтобы сказать об этом так же, как брокер.
    let (tx_rolled_back, settle_err) = if a.write {
        match db::settle_tx(client).await {
            Ok(left) => (left, None),
            Err(e) => (None, Some(e.to_string())),
        }
    } else {
        (None, None)
    };
    let refusal = db::settle_refusal(tx_rolled_back, settle_err.as_deref());
    if !a.no_history {
        let _ = store::record_history(HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            sql: sql.clone(),
            at: store::now_ms(),
            ok: outcome.is_ok() && refusal.is_none(),
        });
    }
    match outcome {
        // Как у сервера сессий: откаченная запись — отказ, а не результат
        // с флагом, чтобы режимы не расходились.
        Ok(_) if refusal.is_some() => {
            let message = refusal.unwrap_or_default();
            report_server_messages(
                &notices.notices,
                notices.dropped,
                tx_rolled_back,
                Some(&message),
            );
            Ok(envelope::fail("refused", &message, &[]))
        }
        Ok(mut exec) => {
            exec.notices = notices.notices;
            exec.notices_dropped = notices.dropped;
            let types = if a.fmt.pick() == Format::Json {
                db::statement_column_types(&connected.session.client, &sql).await
            } else {
                Vec::new()
            };
            render_exec(&a, exec, &types);
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            report_server_messages(&notices.notices, notices.dropped, tx_rolled_back, None);
            let mut hints = Vec::new();
            if let Some(se) = &settle_err {
                hints.push(format!(
                    "sql-kai: состояние транзакции после ошибки не проверено: {se}"
                ));
            }
            if e.is_read_only() {
                hints.push(READ_ONLY_HINT.to_string());
            }
            Ok(envelope::fail(e.code(), &e.to_string(), &hints))
        }
    }
}
