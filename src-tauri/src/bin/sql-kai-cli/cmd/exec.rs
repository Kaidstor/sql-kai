//! `sql-kai exec <ssh-alias>` — fallback без профиля и vault (наследник prod-db):
//! ssh на хост -> найти postgres-контейнер -> docker exec psql. Работает даже
//! когда порт наружу не открыт и пароль неизвестен (psql внутри контейнера
//! ходит по trust/peer). Вывод — сырой текст psql.

use std::path::PathBuf;
use std::process::ExitCode;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use clap::Args;
use sql_kai_lib::error::AppError;

use crate::remote::{self, CONTAINER_DB_ENV, CONTAINER_FIND};
use crate::{input, session};

#[derive(Args)]
pub struct ExecArgs {
    /// SSH-алиас хоста (из ~/.ssh/config*)
    alias: String,
    /// SQL-команда (можно повторять)
    #[arg(short = 'c', long = "command", value_name = "SQL")]
    commands: Vec<String>,
    /// Docker-контейнер postgres (когда на хосте их несколько)
    #[arg(long, value_name = "NAME")]
    container: Option<String>,
    /// Файл с SQL (можно повторять)
    #[arg(short = 'f', long = "file", value_name = "FILE")]
    files: Vec<PathBuf>,
    /// tuples-only без рамок (psql -tA)
    #[arg(short = 't', long)]
    tuples: bool,
    /// Вывод в CSV
    #[arg(long)]
    csv: bool,
    /// Разрешить запись (по умолчанию сессия read-only)
    #[arg(long)]
    write: bool,
    /// Подтвердить запись, если на этот хост смотрит production-профиль
    #[arg(long, requires = "write")]
    prod_write: bool,
    /// Показать резолв контейнера/юзера/базы
    #[arg(short, long)]
    verbose: bool,
    /// Показать команду, не заходя на хост
    #[arg(long)]
    dry_run: bool,
}

/// Хвост поверх [`CONTAINER_FIND`] + [`CONTAINER_DB_ENV`]: заливает переданный SQL (base64 в env) во
/// временный файл и прогоняет его через `docker exec psql` внутри контейнера.
const EXEC_TAIL: &str = r#"if [ -n "${KAI_VERBOSE:-}" ]; then echo "[container=$C user=$U db=$DB]" >&2; fi
SQLF=$(mktemp)
trap 'rm -f "$SQLF"' EXIT
printf '%s' "${KAI_SQL_B64:-}" | base64 -d > "$SQLF"
$D exec -i "$C" psql -U "$U" -d "$DB" -v ON_ERROR_STOP=1 ${KAI_PSQL_OPTS:-} -f - < "$SQLF"
"#;

/// Безопасна ли мета-команда psql на read-only пути. Разрешаем describe-семейство
/// (`\d`…, все они только читают каталог) и переключатели вывода. Всё остальное
/// отвергаем поимённо: `\c`/`\connect` теряет транзакцию, `\i`/`\ir` подключает
/// непроверенный файл, `\gexec` выполняет полученный текст, `\o`/`\g`/`\copy`/`\w`
/// пишут за пределы базы, `\set` меняет поведение самого psql (AUTOCOMMIT).
fn psql_meta_read_only(name: &str) -> bool {
    name.starts_with('d')
        || matches!(
            name,
            "l" | "l+"
                | "z"
                | "sf"
                | "sf+"
                | "sv"
                | "sv+"
                | "x"
                | "a"
                | "t"
                | "f"
                | "H"
                | "C"
                | "pset"
                | "timing"
                | "echo"
                | "qecho"
                | "warn"
                | "conninfo"
                | "encoding"
        )
}

pub fn run(a: ExecArgs) -> Result<ExitCode, AppError> {
    let mut sql = input::collect_sql(&a.commands, &a.files)?;
    let mut psql_opts: Vec<&str> = Vec::new();
    if !a.write {
        // psql выполняет стейтменты по одному, каждый своей транзакцией, поэтому
        // `SET default_transaction_read_only = on` в прологе снимался следующей
        // же строкой батча. Настоящая граница — одна read-only транзакция на
        // весь прогон: --single-transaction оборачивает файл в BEGIN/COMMIT, а
        // первый стейтмент делает её read-only. Внутри неё повысить права
        // нельзя (25001), а выйти из неё не даёт гейт ниже.
        if sql_kai_lib::db::escapes_read_only_tx(&sql) {
            return Err(AppError::Msg(
                "read-only режим: батч вышел бы из read-only транзакции или снял бы с неё \
                 read-only (COMMIT/ROLLBACK/END/ABORT/PREPARE TRANSACTION/DISCARD/\
                 SET TRANSACTION/SET …transaction_read_only, включая форму set_config()). \
                 Добавь --write, если он действительно должен менять данные."
                    .into(),
            ));
        }
        // Read-only транзакция ограничивает запись в базу — и только её. `COPY
        // … TO PROGRAM` запускает шелл на сервере, `COPY … TO '/path'` и
        // lo_export пишут его файловую систему, и ни одно из этого Postgres
        // read-only-флагом не проверяет. Здесь это опаснее всего: psql внутри
        // контейнера работает под POSTGRES_USER, обычно суперюзером.
        if sql_kai_lib::db::reaches_server_side_io(&sql) {
            return Err(AppError::Msg(
                "read-only режим: батч пишет мимо базы — COPY … TO PROGRAM запускает шелл \
                 на сервере, COPY … TO '/путь' и lo_export пишут его диск. Read-only \
                 транзакция это не покрывает. Чтобы забрать данные, используй COPY … TO \
                 STDOUT, а для записи на сервере — --write и осознанное решение."
                    .into(),
            ));
        }
        // Гейт выше видит только SQL, а psql читает из того же потока свои
        // мета-команды: `\c` переподключается (read-only транзакция теряется
        // целиком), `\i` подключает файл, содержимое которого через гейт не
        // проходило, `\gexec` выполняет как SQL то, что вернул запрос, `\!`
        // уходит в шелл контейнера. На read-only пути оставляем только describe
        // и переключатели вывода.
        let forbidden: Vec<String> = sql_kai_lib::db::psql_meta_commands(&sql)
            .into_iter()
            .filter(|name| !psql_meta_read_only(name))
            .collect();
        if !forbidden.is_empty() {
            return Err(AppError::Msg(format!(
                "read-only режим: мета-команды psql (\\{}) выводят батч из read-only \
                 транзакции либо тащат SQL и шелл мимо проверки. Оставь SQL и describe \
                 (\\d…, \\l, \\x, \\timing) или добавь --write, если запись согласована.",
                forbidden.join(", \\")
            )));
        }
        psql_opts.push("--single-transaction");
        sql = format!("SET TRANSACTION READ ONLY;\n{sql}");
    }
    if a.tuples {
        psql_opts.extend(["-t", "-A"]);
    }
    if a.csv {
        psql_opts.push("--csv");
    }
    let env = [
        ("KAI_SQL_B64", B64.encode(&sql)),
        ("KAI_PSQL_OPTS", psql_opts.join(" ")),
        ("KAI_VERBOSE", if a.verbose { "1" } else { "" }.to_string()),
        ("KAI_CONTAINER", a.container.clone().unwrap_or_default()),
        // Неоднозначность контейнера здесь заканчивается отказом, а не выбором
        // первого кандидата: на хосте с двумя кластерами (db_admin + db_app)
        // «первый попавшийся» значит выполнить SQL не в той базе — а на
        // exec-пути это ещё и запись. Предупреждение в stderr для этого слабо:
        // при `--json | jq` его никто не читает. Выход — `--container`.
        ("KAI_STRICT", "1".to_string()),
    ];
    let script = format!("{CONTAINER_FIND}{CONTAINER_DB_ENV}{EXEC_TAIL}");
    let payload = remote::stdin_payload(&script, &env);

    if a.dry_run {
        println!(
            "ssh -T -o BatchMode=yes -o ConnectTimeout=15 -- {} bash -s <<'KAI_EOF'",
            a.alias
        );
        print!("{payload}");
        println!("KAI_EOF");
        return Ok(ExitCode::SUCCESS);
    }

    // Прод-барьер — после --dry-run (тот на хост не ходит) и до ssh: exec
    // пишет мимо профиля, но если на этот ssh-хост заведён production-профиль,
    // это та же боевая база.
    if a.write {
        session::authorize_prod_write_ssh(&a.alias, a.prod_write)?;
    }

    let status = remote::run_via_stdin(&a.alias, &payload)
        .map_err(|e| AppError::Msg(format!("ssh: {e}")))?;
    Ok(ExitCode::from(
        status.code().unwrap_or(1).clamp(0, 255) as u8
    ))
}
