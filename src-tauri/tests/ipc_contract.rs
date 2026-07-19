//! Контракт IPC: фиксирует JSON-форму типов, которые ходят между Rust и
//! webview (invoke-команды) и по сокету брокера. Зеркало — src/lib/types.ts:
//! упал тест — значит, сломан и фронтенд; правь types.ts синхронно.
//!
//! Serialize-тесты сверяют полный JSON (camelCase-ключи, null/пропуски),
//! Deserialize-тесты — что payload фронтенда в его текущей форме читается.

use serde_json::json;

use sql_kai_lib::broker::BrokerSessionInfo;
use sql_kai_lib::commands::{
    ColumnInfo, EnumTypeInfo, IndexInfo, PolicyInfo, RelationInfo, SessionInfo, SortSpec,
    TableColumns, TableInfo, TablePage, TablePolicies, TriggerInfo, VaultStatus,
};
use sql_kai_lib::db::{ExecResult, ExportFormat, ExportOutcome, StatementResult, TxStatus};
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{HistoryEntry, Profile, SavedQuery};

fn to_json<T: serde::Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap()
}

#[test]
fn session_info_shape() {
    let info = SessionInfo {
        session_id: "s1".into(),
        profile_id: "p1".into(),
        server_version: "16.3".into(),
        tunnel_port: Some(5433),
        tx: TxStatus::Idle.as_str().into(),
        isolated: false,
        pid: None,
    };
    assert_eq!(
        to_json(&info),
        json!({
            "sessionId": "s1",
            "profileId": "p1",
            "serverVersion": "16.3",
            "tunnelPort": 5433,
            "tx": "idle",
            "isolated": false,
            "pid": null,
        })
    );
}

/// Значения TxStatus — фронтовый union "idle" | "active" | "failed".
#[test]
fn tx_status_labels() {
    assert_eq!(TxStatus::Idle.as_str(), "idle");
    assert_eq!(TxStatus::Active.as_str(), "active");
    assert_eq!(TxStatus::Failed.as_str(), "failed");
}

#[test]
fn vault_status_shape() {
    let status = VaultStatus {
        exists: true,
        unlocked: false,
        biometric_supported: true,
        biometric_enrolled: false,
    };
    assert_eq!(
        to_json(&status),
        json!({
            "exists": true,
            "unlocked": false,
            "biometricSupported": true,
            "biometricEnrolled": false,
        })
    );
}

#[test]
fn exec_result_shape() {
    let exec = ExecResult {
        results: vec![StatementResult {
            columns: vec!["id".into(), "name".into()],
            rows: vec![vec![Some("1".into()), None]],
            rows_affected: Some(1),
            truncated: false,
        }],
        duration_ms: 5,
    };
    assert_eq!(
        to_json(&exec),
        json!({
            "results": [{
                "columns": ["id", "name"],
                "rows": [["1", null]],
                "rowsAffected": 1,
                "truncated": false,
            }],
            "durationMs": 5,
        })
    );
}

#[test]
fn export_outcome_shape_and_formats() {
    let outcome = ExportOutcome {
        rows: 2,
        truncated: false,
        duration_ms: 7,
    };
    assert_eq!(
        to_json(&outcome),
        json!({ "rows": 2, "truncated": false, "durationMs": 7 })
    );
    // Строки фронтового union ExportFormat принимаются, мусор — нет.
    for fmt in ["csv", "json", "md", "xlsx"] {
        assert!(ExportFormat::parse(fmt).is_ok(), "format {fmt} rejected");
    }
    assert!(ExportFormat::parse("pdf").is_err());
}

#[test]
fn table_page_shape() {
    let page = TablePage {
        result: StatementResult::default(),
        duration_ms: 3,
        approx_rows: -1,
    };
    assert_eq!(
        to_json(&page),
        json!({
            "result": { "columns": [], "rows": [], "rowsAffected": null, "truncated": false },
            "durationMs": 3,
            "approxRows": -1,
        })
    );
}

#[test]
fn introspection_shapes() {
    let table = TableInfo {
        schema: "public".into(),
        name: "users".into(),
        kind: "table".into(),
    };
    assert_eq!(
        to_json(&table),
        json!({ "schema": "public", "name": "users", "kind": "table" })
    );

    let columns = TableColumns {
        schema: "public".into(),
        table: "users".into(),
        columns: vec!["id".into()],
    };
    assert_eq!(
        to_json(&columns),
        json!({ "schema": "public", "table": "users", "columns": ["id"] })
    );

    let column = ColumnInfo {
        name: "id".into(),
        data_type: "bigint".into(),
        nullable: false,
        is_pk: true,
        default_expr: None,
        comment: None,
    };
    assert_eq!(
        to_json(&column),
        json!({
            "name": "id",
            "dataType": "bigint",
            "nullable": false,
            "isPk": true,
            "defaultExpr": null,
            "comment": null,
        })
    );

    let index = IndexInfo {
        name: "users_pkey".into(),
        unique: true,
        primary: true,
        columns: Some("id".into()),
        definition: "CREATE UNIQUE INDEX …".into(),
    };
    assert_eq!(
        to_json(&index),
        json!({
            "name": "users_pkey",
            "unique": true,
            "primary": true,
            "columns": "id",
            "definition": "CREATE UNIQUE INDEX …",
        })
    );

    let relation = RelationInfo {
        name: "orders_user_fk".into(),
        columns: Some("user_id".into()),
        ref_table: "public.users".into(),
        ref_columns: Some("id".into()),
        on_update: "NO ACTION".into(),
        on_delete: "CASCADE".into(),
    };
    assert_eq!(
        to_json(&relation),
        json!({
            "name": "orders_user_fk",
            "columns": "user_id",
            "refTable": "public.users",
            "refColumns": "id",
            "onUpdate": "NO ACTION",
            "onDelete": "CASCADE",
        })
    );

    let trigger = TriggerInfo {
        name: "trg".into(),
        timing: "BEFORE".into(),
        events: "INSERT".into(),
        definition: "CREATE TRIGGER …".into(),
        enabled: true,
    };
    assert_eq!(
        to_json(&trigger),
        json!({
            "name": "trg",
            "timing": "BEFORE",
            "events": "INSERT",
            "definition": "CREATE TRIGGER …",
            "enabled": true,
        })
    );

    let enum_type = EnumTypeInfo {
        schema: "public".into(),
        name: "status".into(),
        labels: vec!["new".into(), "done".into()],
    };
    assert_eq!(
        to_json(&enum_type),
        json!({ "schema": "public", "name": "status", "labels": ["new", "done"] })
    );

    let policies = TablePolicies {
        rls_enabled: true,
        rls_forced: false,
        policies: vec![PolicyInfo {
            name: "tenant_read".into(),
            command: "SELECT".into(),
            permissive: true,
            roles: None,
            using_expr: Some("tenant_id = 1".into()),
            check_expr: None,
        }],
    };
    assert_eq!(
        to_json(&policies),
        json!({
            "rlsEnabled": true,
            "rlsForced": false,
            "policies": [{
                "name": "tenant_read",
                "command": "SELECT",
                "permissive": true,
                "roles": null,
                "usingExpr": "tenant_id = 1",
                "checkExpr": null,
            }],
        })
    );
}

/// AppError уходит как {code, message}; по кодам connection_lost/session_gone
/// фронт (isSessionLost в api.ts) предлагает reconnect — они и зафиксированы.
#[test]
fn app_error_shape() {
    let err = to_json(&AppError::SessionGone);
    assert_eq!(err["code"], "session_gone");
    assert!(err["message"].is_string());
    assert_eq!(to_json(&AppError::ConnectionLost)["code"], "connection_lost");
    assert_eq!(to_json(&AppError::Msg("boom".into())), json!({ "code": "app", "message": "boom" }));
}

/// list_cli_sessions / метод sessions брокера → CliSessionInfo в types.ts.
#[test]
fn broker_session_info_shape() {
    let info = BrokerSessionInfo {
        profile_id: "p1".into(),
        profile_name: "prod".into(),
        origin: "cli".into(),
        server_version: "16.3".into(),
        tunnel_port: None,
        tx: "idle".into(),
        idle_sec: Some(30),
    };
    assert_eq!(
        to_json(&info),
        json!({
            "profileId": "p1",
            "profileName": "prod",
            "origin": "cli",
            "serverVersion": "16.3",
            "tunnelPort": null,
            "tx": "idle",
            "idleSec": 30,
        })
    );
}

/// get_table_page принимает sorts в форме SortSpec[] из api.ts.
#[test]
fn sort_spec_accepts_frontend_payload() {
    let sort: SortSpec = serde_json::from_value(json!({ "column": "id", "dir": "desc" })).unwrap();
    assert_eq!(sort.column, "id");
    assert_eq!(sort.dir.as_deref(), Some("desc"));
    // dir опционален (бэкенд трактует не-"desc" как ASC)
    let bare: SortSpec = serde_json::from_value(json!({ "column": "name" })).unwrap();
    assert!(bare.dir.is_none());
}

/// Профиль: полный round-trip payload'а ConnectionDialog — что фронт прислал,
/// то (плюс дефолты и минус skip-поля) он и получает назад из list_profiles.
#[test]
fn profile_roundtrip() {
    let from_frontend = json!({
        "id": "p1",
        "name": "prod",
        "host": "db.internal",
        "port": 5432,
        "database": "app",
        "user": "me",
        "ssh": {
            "host": "bastion",
            "user": "root",
            "port": 22,
            "keyPath": "~/.ssh/id_ed25519",
            "keepaliveInterval": 5,
        },
        "ssl": { "enabled": true, "caCert": "/ca.pem", "rejectUnauthorized": true },
        "group": "ms",
        "color": "rose",
        "production": true,
        "hasPassword": true,
        "hasSshPassphrase": false,
    });
    let profile: Profile = serde_json::from_value(from_frontend.clone()).unwrap();
    assert_eq!(to_json(&profile), from_frontend);
}

/// Минимальный профиль (новое подключение): опциональные поля не обязаны
/// приходить; в ответе ssh/has_* присутствуют явно, skip-поля опущены.
#[test]
fn profile_minimal_defaults() {
    let profile: Profile = serde_json::from_value(json!({
        "name": "local",
        "host": "127.0.0.1",
        "port": 5432,
        "database": "postgres",
        "user": "postgres",
    }))
    .unwrap();
    assert_eq!(
        to_json(&profile),
        json!({
            "id": "",
            "name": "local",
            "host": "127.0.0.1",
            "port": 5432,
            "database": "postgres",
            "user": "postgres",
            "ssh": null,
            "hasPassword": false,
            "hasSshPassphrase": false,
        })
    );
}

#[test]
fn history_entry_roundtrip() {
    let wire = json!({
        "id": "h1",
        "profileId": "p1",
        "profileName": "prod",
        "sql": "SELECT 1",
        "at": 1_700_000_000_000_i64,
        "ok": true,
    });
    let entry: HistoryEntry = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(to_json(&entry), wire);
}

#[test]
fn saved_query_roundtrip() {
    let wire = json!({
        "id": "q1",
        "name": "orders per day",
        "sql": "SELECT count(*) FROM orders",
        "scope": null,
    });
    let query: SavedQuery = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(to_json(&query), wire);
}
