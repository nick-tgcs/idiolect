use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_sqlite::SqliteMetadataStore;
use serde_json::Value;

#[test]
fn idiolectd_version_reports_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_idiolectd"))
        .args(["--version", "--json"])
        .output()
        .expect("idiolectd should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be json");

    assert_eq!(json["name"], "idiolectd");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["protocol_version"], 1);
}

#[test]
fn idiolectd_fixture_once_commits_to_temp_database() {
    let db_path = unique_temp_db_path("fixture-once-commit");

    let output = Command::new(env!("CARGO_BIN_EXE_idiolectd"))
        .args([
            "fixture-once",
            "--db",
            db_path.to_str().expect("temp db path should be utf8"),
            "--transcript",
            "restart traffic",
            "--corrected",
            "restart Traefik",
            "--commit",
        ])
        .output()
        .expect("idiolectd fixture-once should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be json");
    assert_eq!(json["committed"], true);
    assert_eq!(json["cancelled"], false);
    assert_eq!(json["text"], "restart Traefik");

    let store = open_store(&db_path);
    assert_eq!(
        store
            .training_candidate_count_for_test()
            .expect("candidate count should query"),
        1
    );
    assert_eq!(store.event_count_for_test().expect("event count"), 3);

    cleanup_db_path(db_path);
}

#[test]
fn idiolectd_fixture_once_cancel_records_no_candidate() {
    let db_path = unique_temp_db_path("fixture-once-cancel");

    let output = Command::new(env!("CARGO_BIN_EXE_idiolectd"))
        .args([
            "fixture-once",
            "--db",
            db_path.to_str().expect("temp db path should be utf8"),
            "--transcript",
            "open notes",
            "--cancel",
        ])
        .output()
        .expect("idiolectd fixture-once should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be json");
    assert_eq!(json["committed"], false);
    assert_eq!(json["cancelled"], true);
    assert_eq!(json["text"], "open notes");

    let store = open_store(&db_path);
    assert_eq!(
        store
            .training_candidate_count_for_test()
            .expect("candidate count should query"),
        0
    );
    assert_eq!(store.event_count_for_test().expect("event count"), 2);

    cleanup_db_path(db_path);
}

fn open_store(path: &PathBuf) -> SqliteMetadataStore {
    let mut store = SqliteMetadataStore::open_path(path).expect("database should open");
    store.migrate().expect("migrations should run");
    store
}

fn unique_temp_db_path(tag: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    env::temp_dir().join(format!(
        "idiolectd-{tag}-{}-{}.db",
        std::process::id(),
        now.as_nanos()
    ))
}

fn cleanup_db_path(path: PathBuf) {
    let _ = fs::remove_file(path);
}
