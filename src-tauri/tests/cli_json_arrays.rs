//! Against an isolated loopback PostgreSQL with trust authentication:
//! SQL_KAI_TEST_PORT=<port> SQL_KAI_TEST_ARTIFACTS=$(mktemp -d /tmp/sql-arrays.XXXXXX) \
//! cargo test --features cli --test cli_json_arrays -- --ignored --nocapture
#![cfg(all(feature = "cli", unix))]

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use sql_kai_lib::{broker, vault};

const SQL: &str = r#"
SELECT ARRAY['185.71.67.15']::text[] AS binding_ips,
       ARRAY['https','http']::text[] AS schemes,
       '{}'::text[] AS empty_array, NULL::text[] AS null_array,
       ARRAY[NULL,'NULL','','a,b','a"b',E'a\\b','{a,b}',' пробел ',E'a\nb']::text[] AS quoted,
       ARRAY[1,NULL,-2]::int[] AS ints, ARRAY[true,false,NULL]::bool[] AS bools,
       ARRAY[9223372036854775807,-9223372036854775808]::bigint[] AS bigints,
       ARRAY[9007199254740993.1234567890123456789,42,1.5,'NaN'::numeric] AS numerics,
       ARRAY[[1,NULL],[3,4]]::int[] AS matrix,
       '[0:1][-1:0]={{a,b},{c,d}}'::text[] AS bounds,
       '{a,b}'::text AS text_scalar,
       9007199254740993.1234567890123456789::numeric AS numeric_scalar,
       ARRAY['{"a":1}','[1,2]',NULL]::jsonb[] AS json_values,
       ARRAY['(3,4),(1,2)'::box,'(7,8),(5,6)'::box] AS boxes;
"#;

async fn cli(config: &Path, args: &[&str]) -> std::process::Output {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_sql-kai-cli"))
            .env("SQL_KAI_CONFIG_DIR", config)
            .env_remove("SQL_KAI_VAULT_PASSWORD")
            .args(args)
            .output(),
    )
    .await
    .expect("CLI timeout")
    .expect("CLI spawn");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a dedicated loopback PostgreSQL with trust authentication"]
async fn arrays_through_local_and_shared_broker_server() {
    let port: u16 = std::env::var("SQL_KAI_TEST_PORT")
        .expect("SQL_KAI_TEST_PORT")
        .parse()
        .unwrap();
    let root = std::env::var_os("SQL_KAI_TEST_ARTIFACTS")
        .map(std::path::PathBuf::from)
        .expect("SQL_KAI_TEST_ARTIFACTS must point to your mktemp directory");
    let config = root.join(format!("config-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    std::fs::create_dir(&config).unwrap();
    std::env::set_var("SQL_KAI_CONFIG_DIR", &config);
    let profile = root.join("integration-profile.json");
    std::fs::write(
        &profile,
        json!([{
            "name":"array-test", "host":"127.0.0.1", "port":port,
            "database":"postgres", "user":"postgres"
        }])
        .to_string(),
    )
    .unwrap();
    cli(
        &config,
        &["import", "--no-passwords", "-f", profile.to_str().unwrap()],
    )
    .await;
    let query = root.join("arrays.sql");
    std::fs::write(&query, SQL).unwrap();
    let args = [
        "q",
        "array-test",
        "--no-history",
        "--json",
        "-f",
        query.to_str().unwrap(),
    ];
    let mut local_args = args.to_vec();
    local_args.push("--local");
    let local = cli(&config, &local_args).await;
    std::fs::write(root.join("green-local.json"), &local.stdout).unwrap();
    let local: Value = serde_json::from_slice(&local.stdout).unwrap();
    assert_eq!(local["exit"], 0);
    assert_eq!(local["error"], Value::Null);
    assert!(local.get("warning").is_none(), "{local}");
    let expected = json!([
        ["185.71.67.15"], ["https","http"], [], null,
        [null,"NULL","","a,b","a\"b","a\\b","{a,b}"," пробел ","a\nb"],
        [1,null,-2], [true,false,null], [i64::MAX,i64::MIN],
        [serde_json::from_str::<Value>("9007199254740993.1234567890123456789").unwrap(),42,1.5,"NaN"],
        [[1,null],[3,4]], [["a","b"],["c","d"]], "{a,b}",
        serde_json::from_str::<Value>("9007199254740993.1234567890123456789").unwrap(),
        [{"a":1},[1,2],null], ["(3,4),(1,2)","(7,8),(5,6)"]
    ]);
    assert_eq!(local["data"]["results"][0]["rows"][0], expected);

    // Set up an isolated in-memory vault: CLI trust would touch the user's keychain.
    vault::setup(&uuid::Uuid::new_v4().to_string()).unwrap();
    let hooks = Arc::new(broker::BrokerHooks {
        gui_sessions: Box::new(Vec::new),
        changed: Box::new(|| {}),
        profiles_changed: Box::new(|| {}),
        shutdown: None,
        open_in_gui: None,
        gui_selection: None,
    });
    for (socket, via) in [
        ("holder.sock", "через holder"),
        ("broker.sock", "через брокер GUI"),
    ] {
        let listener = tokio::net::UnixListener::bind(config.join(socket)).unwrap();
        let server = tokio::spawn(broker::serve(listener, Arc::default(), hooks.clone()));
        let mut broker_args = args.to_vec();
        broker_args.push("--verbose");
        let result = cli(&config, &broker_args).await;
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains(via),
            "must use the broker without direct fallback: {stderr}"
        );
        std::fs::write(root.join(format!("green-{socket}.json")), &result.stdout).unwrap();
        let result: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(result["data"]["results"], local["data"]["results"]);
        assert!(result.get("warning").is_none());
        server.abort();
        std::fs::remove_file(config.join(socket)).unwrap();
    }
    println!(
        "15 columns: exact JSON through --local, holder and GUI broker sockets; artifacts: {}",
        root.display()
    );
}
