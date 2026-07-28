//! End-to-end test of the DB layer against a real Postgres.
//! Requires: docker run -d --name sqltauri-test-pg -e POSTGRES_PASSWORD=testpw \
//!   -e POSTGRES_DB=demo -p 54329:5432 postgres:16-alpine
//! Run with: cargo test --test db_integration -- --ignored

use sql_kai_lib::db;
use sql_kai_lib::store::Profile;

fn test_profile() -> Profile {
    Profile {
        id: "test".into(),
        name: "test".into(),
        host: "127.0.0.1".into(),
        port: 54329,
        database: "demo".into(),
        user: "postgres".into(),
        ssh: None,
        group: None,
        color: None,
        production: false,
        ssl: None, // plaintext (and 127.0.0.1 is loopback anyway)
        fork_container: None,
        has_password: false,
        has_ssh_passphrase: false,
        last_connected: None,
    }
}

#[tokio::test]
#[ignore]
async fn connect_execute_paginate() {
    // the password rides in as an override — the vault stays out of the test
    let connected = db::connect(
        &test_profile(),
        db::ConnectOptions {
            password_override: Some("testpw".into()),
            ..Default::default()
        },
    )
    .await
    .expect("connect");
    assert!(!connected.server_version.is_empty());
    let client = connected.session.client.clone();

    // DDL + seed + select in one multi-statement batch (CASCADE: t_view
    // from a previous run depends on t)
    let exec = db::execute(
        &client,
        "DROP TABLE IF EXISTS t CASCADE;
         CREATE TABLE t (id serial PRIMARY KEY, name text, meta jsonb DEFAULT '{}'::jsonb, ts timestamptz DEFAULT now());
         INSERT INTO t (name, meta) SELECT 'row ' || g, jsonb_build_object('n', g) FROM generate_series(1, 250) g;
         CREATE VIEW t_view AS SELECT id, name, ts FROM t WHERE id > 200;
         SELECT id, name, ts, NULL::text AS nothing FROM t ORDER BY id",
        100,
    )
    .await
    .expect("execute batch");

    assert_eq!(exec.results.len(), 5);
    let select = &exec.results[4];
    assert_eq!(select.columns, vec!["id", "name", "ts", "nothing"]);
    assert_eq!(select.rows.len(), 100, "capped at max_rows");
    assert!(select.truncated);
    assert_eq!(select.rows[0][0].as_deref(), Some("1"));
    assert_eq!(select.rows[0][1].as_deref(), Some("row 1"));
    assert_eq!(select.rows[0][3], None, "NULL comes through as None");

    // error path: broken SQL reports position
    let err = db::execute(&client, "SELEC 1", 10).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("syntax error"), "got: {msg}");

    // pagination-style query built the same way get_table_page does
    let page = db::execute(
        &client,
        &format!(
            "SELECT * FROM {} ORDER BY {} DESC LIMIT 10 OFFSET 20",
            db::quote_ident("t"),
            db::quote_ident("id"),
        ),
        10,
    )
    .await
    .expect("page");
    assert_eq!(page.results[0].rows.len(), 10);
    assert_eq!(page.results[0].rows[0][0].as_deref(), Some("230"));

    // empty result set still yields column names via RowDescription
    let empty = db::execute(&client, "SELECT id, name FROM t WHERE false", 10)
        .await
        .expect("empty");
    assert_eq!(empty.results[0].columns, vec!["id", "name"]);
    assert!(empty.results[0].rows.is_empty());

    // introspection queries used by the Structure tab
    let regclass = db::quote_literal("\"public\".\"t\"");

    let cols = db::execute(&client, &db::columns_sql(&regclass), 100)
        .await
        .expect("columns");
    let rows = &cols.results[0].rows;
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0][0].as_deref(), Some("id"));
    assert_eq!(rows[0][3].as_deref(), Some("true"), "id is pk");
    assert!(
        rows[0][4].as_deref().unwrap_or("").contains("nextval"),
        "serial default"
    );
    assert_eq!(rows[1][2].as_deref(), Some("true"), "name is nullable");
    assert_eq!(rows[2][0].as_deref(), Some("meta"));
    assert_eq!(rows[2][1].as_deref(), Some("jsonb"), "declared type comes through");

    let idx = db::execute(&client, &db::indexes_sql(&regclass), 100)
        .await
        .expect("indexes");
    let idx_rows = &idx.results[0].rows;
    assert_eq!(idx_rows[0][0].as_deref(), Some("t_pkey"));
    assert_eq!(idx_rows[0][2].as_deref(), Some("true"), "primary");
    assert_eq!(idx_rows[0][3].as_deref(), Some("id"));

    // views: pageable exactly like tables (get_table_page path), and
    // introspection reports no primary key — the grid goes read-only
    let vreg = db::quote_literal("\"public\".\"t_view\"");
    let vcols = db::execute(&client, &db::columns_sql(&vreg), 100)
        .await
        .expect("view columns");
    let vrows = &vcols.results[0].rows;
    assert_eq!(vrows.len(), 3);
    assert!(
        vrows.iter().all(|r| r[3].as_deref() == Some("false")),
        "views have no pk"
    );
    let vpage = db::execute(
        &client,
        &format!(
            "SELECT * FROM {} ORDER BY {} DESC LIMIT 10 OFFSET 0",
            db::quote_ident("t_view"),
            db::quote_ident("id"),
        ),
        10,
    )
    .await
    .expect("view page");
    assert_eq!(vpage.results[0].rows.len(), 10);
    assert_eq!(vpage.results[0].rows[0][0].as_deref(), Some("250"));

    // relations / triggers: none on t, but the SQL must be valid
    let rel = db::execute(&client, &db::relations_sql(&regclass), 100)
        .await
        .expect("relations");
    assert!(rel.results[0].rows.is_empty());
    let trg = db::execute(&client, &db::triggers_sql(&regclass), 100)
        .await
        .expect("triggers");
    assert!(trg.results[0].rows.is_empty());

    // column-editing DDL round-trip (what the Structure UI generates)
    db::execute(
        &client,
        "ALTER TABLE \"public\".\"t\" ADD COLUMN \"note\" text DEFAULT 'hi'",
        10,
    )
    .await
    .expect("add column");
    db::execute(
        &client,
        "ALTER TABLE \"public\".\"t\" RENAME COLUMN \"note\" TO \"remark\"",
        10,
    )
    .await
    .expect("rename column");
    let after = db::execute(&client, &db::columns_sql(&regclass), 100)
        .await
        .expect("columns after ddl");
    let names: Vec<_> = after.results[0]
        .rows
        .iter()
        .map(|r| r[0].as_deref().unwrap_or(""))
        .collect();
    assert!(names.contains(&"remark"), "got: {names:?}");

    // data-editing batch (what the table grid's Apply generates):
    // no explicit BEGIN — a multi-statement simple query IS one implicit tx
    db::execute(
        &client,
        "UPDATE \"public\".\"t\" SET \"name\" = 'edited' WHERE \"id\" = '1';\n\
         DELETE FROM \"public\".\"t\" WHERE \"id\" = '2'",
        10,
    )
    .await
    .expect("apply edits batch");
    let check = db::execute(
        &client,
        "SELECT name FROM t WHERE id = 1; SELECT count(*) FROM t WHERE id = 2",
        10,
    )
    .await
    .expect("check edits");
    assert_eq!(check.results[0].rows[0][0].as_deref(), Some("edited"));
    assert_eq!(check.results[1].rows[0][0].as_deref(), Some("0"));

    // jsonb round-trip: the cell JSON editor stages a compact json literal
    db::execute(
        &client,
        "UPDATE \"public\".\"t\" SET \"meta\" = '{\"tags\":[\"a\",\"b\"],\"level\":3}' WHERE \"id\" = '1'",
        10,
    )
    .await
    .expect("update jsonb");
    let meta = db::execute(&client, "SELECT meta FROM t WHERE id = 1", 10)
        .await
        .expect("select jsonb");
    let text = meta.results[0].rows[0][0].as_deref().unwrap_or("");
    assert!(text.contains("\"level\": 3"), "jsonb text output: {text}");
    assert!(text.contains("\"tags\""), "jsonb text output: {text}");

    // a failing statement rolls back the whole implicit transaction
    // and must NOT leave the session in an aborted-tx state
    let err = db::execute(
        &client,
        "UPDATE \"public\".\"t\" SET \"name\" = 'lost' WHERE \"id\" = '3';\n\
         UPDATE \"public\".\"t\" SET \"nope\" = '1' WHERE \"id\" = '3'",
        10,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("nope"), "got: {err}");
    let intact = db::execute(&client, "SELECT name FROM t WHERE id = 3", 10)
        .await
        .expect("session usable right after failed batch");
    assert_eq!(intact.results[0].rows[0][0].as_deref(), Some("row 3"));
}

/// `schema --table` narrowing against a real catalog: the six relation-scoped
/// result sets must carry the named table only, and the type/routine/sequence
/// ones must carry exactly what that table uses. Both are easy to get subtly
/// wrong (an enum behind an array column hides in `typelem`, a trigger function
/// lives in another catalog), and neither shows up in a SQL-text unit test.
#[tokio::test]
#[ignore]
async fn schema_dump_narrowed_to_one_table() {
    let connected = db::connect(
        &test_profile(),
        db::ConnectOptions {
            password_override: Some("testpw".into()),
            ..Default::default()
        },
    )
    .await
    .expect("connect");
    let client = connected.session.client.clone();

    db::execute(
        &client,
        "DROP TABLE IF EXISTS scoped CASCADE;
         DROP TABLE IF EXISTS unscoped CASCADE;
         DROP TYPE IF EXISTS scoped_state CASCADE;
         DROP TYPE IF EXISTS lonely_state CASCADE;
         DROP FUNCTION IF EXISTS scoped_touch() CASCADE;
         DROP SEQUENCE IF EXISTS lonely_seq CASCADE;
         CREATE TYPE scoped_state AS ENUM ('new', 'done');
         CREATE TYPE lonely_state AS ENUM ('nobody');
         CREATE SEQUENCE lonely_seq;
         CREATE TABLE scoped (
           id serial PRIMARY KEY,
           states scoped_state[] NOT NULL DEFAULT '{}'::scoped_state[]
         );
         CREATE TABLE unscoped (id int);
         CREATE FUNCTION scoped_touch() RETURNS trigger LANGUAGE plpgsql
           AS $$ BEGIN RETURN NEW; END $$;
         CREATE TRIGGER scoped_trg BEFORE UPDATE ON scoped
           FOR EACH ROW EXECUTE FUNCTION scoped_touch()",
        10,
    )
    .await
    .expect("fixture");

    let opts = db::SchemaOptions {
        table: Some("scoped".into()),
        ..Default::default()
    };
    let dump = db::execute(&client, &db::schema_dump_sql(&opts), 10_000)
        .await
        .expect("scoped dump");
    assert_eq!(dump.results.len(), db::SCHEMA_DUMP_PARTS);
    let names = |part: usize, col: usize| -> Vec<String> {
        dump.results[part]
            .rows
            .iter()
            .map(|r| r[col].clone().unwrap_or_default())
            .collect()
    };

    assert_eq!(names(0, 1), vec!["scoped"], "relations");
    assert_eq!(names(1, 2), vec!["id", "states"], "columns");
    assert_eq!(names(4, 2), vec!["scoped_trg"], "triggers");
    // one row per label — the enum sits behind `scoped_state[]`, so matching
    // atttypid alone would have lost it entirely
    assert_eq!(names(5, 1), vec!["scoped_state", "scoped_state"], "enum types");
    assert_eq!(names(5, 2), vec!["new", "done"], "enum labels");
    assert_eq!(names(6, 1), vec!["scoped_touch"], "routines");
    // the whole-database dump prints sequences nobody owns; scoped to a table
    // it is the other way round — only the serial's own sequence
    assert_eq!(names(8, 1), vec!["scoped_id_seq"], "sequences");

    let all = db::execute(
        &client,
        &db::schema_dump_sql(&db::SchemaOptions::default()),
        10_000,
    )
    .await
    .expect("whole dump");
    assert!(all.results[0].rows.len() > dump.results[0].rows.len());
    let seqs: Vec<String> = all.results[8]
        .rows
        .iter()
        .map(|r| r[1].clone().unwrap_or_default())
        .collect();
    assert!(seqs.contains(&"lonely_seq".to_string()), "got: {seqs:?}");
    assert!(!seqs.contains(&"scoped_id_seq".to_string()), "got: {seqs:?}");
}

/// Прод-барьер GUI: без write-intent батч идёт через `execute_read_only`, и
/// записью считается то, что считает записью сам Postgres. Ровно те классы,
/// которые regex-классификатор фронта пропускал (CREATE, CALL, DO, пишущая
/// CTE, EXPLAIN ANALYZE UPDATE), обязаны падать с 25006; чтения — проходить.
#[tokio::test]
#[ignore]
async fn read_only_block_refuses_what_the_regex_missed() {
    let connected = db::connect(
        &test_profile(),
        db::ConnectOptions {
            password_override: Some("testpw".into()),
            ..Default::default()
        },
    )
    .await
    .expect("connect");
    let client = connected.session.client.clone();

    db::execute(
        &client,
        "DROP TABLE IF EXISTS guard_t CASCADE;
         CREATE TABLE guard_t (id int);
         INSERT INTO guard_t VALUES (1);
         CREATE OR REPLACE PROCEDURE guard_write() LANGUAGE sql
           AS $$ INSERT INTO guard_t VALUES (99) $$",
        10,
    )
    .await
    .expect("fixture");

    let refused = [
        "CREATE TABLE guard_new (id int)",
        "CREATE INDEX guard_idx ON guard_t (id)",
        "CALL guard_write()",
        "DO $$ BEGIN INSERT INTO guard_t VALUES (7); END $$",
        "WITH w AS (INSERT INTO guard_t VALUES (8) RETURNING id) SELECT * FROM w",
        "EXPLAIN (ANALYZE, FORMAT JSON) UPDATE guard_t SET id = id + 1",
        "INSERT INTO guard_t VALUES (2)",
    ];
    for sql in refused {
        let err = db::execute_read_only(&client, sql, 10)
            .await
            .expect_err(sql);
        assert!(err.is_read_only(), "{sql}: expected 25006, got: {err}");
    }
    // отказы не оставили ни изменений, ни открытой транзакции
    let count = db::query_scalar(&client, "SELECT count(*) FROM guard_t")
        .await
        .expect("count")
        .unwrap_or_default();
    assert_eq!(count, "1", "no write leaked through the read-only block");

    let ok = db::execute_read_only(
        &client,
        "SELECT id FROM guard_t; EXPLAIN (FORMAT JSON) UPDATE guard_t SET id = 0",
        10,
    )
    .await
    .expect("reads and plain EXPLAIN pass");
    assert_eq!(ok.results.len(), 2);

    // гейт до отправки: батч, выходящий из блока, отвергается тем же кодом
    let err = db::execute_read_only(&client, "COMMIT; DELETE FROM guard_t", 10)
        .await
        .expect_err("escape refused");
    assert!(err.is_read_only(), "gate refusal shares the read_only code: {err}");
}

/// Экспорт обязан дочитывать протокол до конца: ошибка стейтмента ПОСЛЕ
/// экспортируемого — это ошибка экспорта, а не «успех» с потерянным хвостом;
/// хвостовые DDL исполняются и должны быть видны; недоступный файл не должен
/// запускать скрипт вовсе (SQL после создания файла).
#[tokio::test]
#[ignore]
async fn export_drains_the_whole_script() {
    let connected = db::connect(
        &test_profile(),
        db::ConnectOptions {
            password_override: Some("testpw".into()),
            ..Default::default()
        },
    )
    .await
    .expect("connect");
    let client = connected.session.client.clone();
    let path = |name: &str| {
        std::env::temp_dir()
            .join(format!("sql-kai-it-{name}-{}.csv", std::process::id()))
            .to_string_lossy()
            .into_owned()
    };

    db::execute(
        &client,
        "DROP TABLE IF EXISTS exp_t CASCADE; DROP TABLE IF EXISTS exp_late CASCADE;
         DROP TABLE IF EXISTS exp_never CASCADE;
         CREATE TABLE exp_t (id int); INSERT INTO exp_t VALUES (1), (2)",
        10,
    )
    .await
    .expect("fixture");

    // ошибка после экспортируемого стейтмента — Err, не молчаливый успех
    let p = path("late-err");
    let err = db::export_statement(
        &client,
        "SELECT id FROM exp_t; SELECT missing_column FROM exp_t",
        0,
        db::ExportFormat::parse("csv").unwrap(),
        &p,
    )
    .await
    .expect_err("late error must surface");
    let msg = sql_kai_lib::error::AppError::from(err).to_string();
    assert!(msg.contains("missing_column"), "got: {msg}");
    let _ = std::fs::remove_file(&p);

    // хвост скрипта исполняется и виден после успешного экспорта
    let p = path("late-ddl");
    let out = db::export_statement(
        &client,
        "SELECT id FROM exp_t ORDER BY id; CREATE TABLE exp_late (id int)",
        0,
        db::ExportFormat::parse("csv").unwrap(),
        &p,
    )
    .await
    .expect("export with trailing DDL");
    assert_eq!(out.rows, 2);
    let text = std::fs::read_to_string(&p).expect("file written");
    assert!(text.starts_with("id"), "got: {text}");
    let _ = std::fs::remove_file(&p);
    let late = db::query_scalar(
        &client,
        "SELECT count(*) FROM pg_class WHERE relname = 'exp_late'",
    )
    .await
    .expect("late ddl check");
    assert_eq!(late.as_deref(), Some("1"), "trailing DDL ran to completion");

    // файл раньше SQL: несоздаваемый путь — скрипт не выполнялся
    let err = db::export_statement(
        &client,
        "CREATE TABLE exp_never (id int); SELECT 1",
        1,
        db::ExportFormat::parse("csv").unwrap(),
        "/nonexistent-dir/never.csv",
    )
    .await
    .expect_err("unwritable path");
    assert!(
        matches!(err, db::ExportError::Local(_)),
        "local error, SQL never sent"
    );
    let never = db::query_scalar(
        &client,
        "SELECT count(*) FROM pg_class WHERE relname = 'exp_never'",
    )
    .await
    .expect("never-ran check");
    assert_eq!(never.as_deref(), Some("0"), "doomed export must not run the script");
}

/// Verifies the PostgreSQL semantics the broker's read-only gate defends
/// against: `default_transaction_read_only` applies only to a NEW transaction,
/// so a read-write transaction left open by a `--write` call stays writable
/// even after the default is flipped back on. That is exactly the hole the
/// broker now closes by tracking the open transaction's access mode
/// (`Session::tx_write`) and refusing read-only calls inside a read-write tx.
#[tokio::test]
#[ignore]
async fn read_only_default_does_not_cover_open_transaction() {
    let connected = db::connect(
        &test_profile(),
        db::ConnectOptions {
            password_override: Some("testpw".into()),
            ..Default::default()
        },
    )
    .await
    .expect("connect");
    let client = connected.session.client.clone();

    db::execute(
        &client,
        "DROP TABLE IF EXISTS ro_probe; CREATE TABLE ro_probe (id int)",
        10,
    )
    .await
    .expect("setup");
    db::execute(&client, "INSERT INTO ro_probe VALUES (1)", 10)
        .await
        .expect("seed");

    // Broker baseline: session is read-only by default.
    db::execute(&client, "SET default_transaction_read_only = on", 1)
        .await
        .expect("ro on");
    // A --write call: flip the default off, open a tx, write, flip it back on —
    // this is what leaves a read-write transaction open across broker calls.
    db::execute(&client, "SET default_transaction_read_only = off", 1)
        .await
        .expect("ro off");
    db::execute(&client, "BEGIN", 1).await.expect("begin");
    db::execute(&client, "INSERT INTO ro_probe VALUES (2)", 10)
        .await
        .expect("write inside tx");
    db::execute(&client, "SET default_transaction_read_only = on", 1)
        .await
        .expect("ro on again");

    // The still-open transaction remains read-write despite the default: the
    // exact condition the broker gate detects (before != Idle && tx_write).
    let flag = db::execute(&client, "SHOW transaction_read_only", 1)
        .await
        .expect("show");
    assert_eq!(
        flag.results[0].rows[0][0].as_deref(),
        Some("off"),
        "leftover tx stays read-write even though default_transaction_read_only=on"
    );
    // Without the gate a plain (read-only) call would DELETE here — proving the
    // guarantee needs the transaction-aware gate, not just the session default.
    db::execute(&client, "DELETE FROM ro_probe WHERE id = 1", 10)
        .await
        .expect("delete slips through the leftover read-write tx");
    db::execute(&client, "ROLLBACK", 1).await.expect("rollback");
}
