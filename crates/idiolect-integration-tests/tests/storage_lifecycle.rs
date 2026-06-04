use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ports::storage::MetadataStorePort;

#[test]
fn committed_session_writes_event_then_materialized_rows() {
    let path = unique_temp_db_path("committed-session");
    let mut store = SqliteMetadataStore::open_path(&path).expect("database should open");

    store.migrate().expect("store migration should succeed");

    let store_port: &mut dyn MetadataStorePort<
        Error = idiolect_adapter_sqlite::SqliteStorageError,
    > = &mut store;

    let session_id = store_port
        .create_session(Some("restart traffic"))
        .expect("session should be created");
    store_port
        .commit_session(session_id, "restart Traefik", "lifecycle-commit-1")
        .expect("session commit should succeed");

    assert_eq!(
        store
            .event_count_for_test()
            .expect("event count should query"),
        2
    );
    assert_eq!(
        store
            .training_candidate_count_for_test()
            .expect("candidate count should query"),
        1
    );
    assert_eq!(
        store
            .session_state_for_test(session_id)
            .expect("session state should be readable"),
        "committed"
    );

    cleanup_db_path(path);
}

#[test]
fn lifecycle_commit_is_replay_consistent_after_restart() {
    let path = unique_temp_db_path("lifecycle-restart");

    let session_id = {
        let mut store = SqliteMetadataStore::open_path(&path).expect("database should open");
        store.migrate().expect("store migration should succeed");

        let store_port: &mut dyn MetadataStorePort<
            Error = idiolect_adapter_sqlite::SqliteStorageError,
        > = &mut store;

        let session_id = store_port
            .create_session(Some("restart traffic"))
            .expect("session should be created");
        store_port
            .commit_session(session_id, "restart Traefik", "lifecycle-commit-1")
            .expect("session commit should succeed");
        assert_eq!(
            store
                .event_count_for_test()
                .expect("event count should query"),
            2
        );
        assert_eq!(
            store
                .training_candidate_count_for_test()
                .expect("candidate count should query"),
            1
        );
        assert_eq!(
            store
                .session_state_for_test(session_id)
                .expect("session state should be readable"),
            "committed"
        );
        session_id
    };

    let mut reopened = SqliteMetadataStore::open_path(&path).expect("database should reopen");
    reopened
        .migrate()
        .expect("reopen migration should remain idempotent");
    assert_eq!(
        reopened
            .event_count_for_test()
            .expect("event count should query"),
        2
    );
    assert_eq!(
        reopened
            .training_candidate_count_for_test()
            .expect("candidate count should query"),
        1
    );
    assert_eq!(
        reopened
            .session_state_for_test(session_id)
            .expect("session state should be readable"),
        "committed"
    );

    cleanup_db_path(path);
}

fn unique_temp_db_path(tag: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    env::temp_dir().join(format!(
        "idiolect-storage-lifecycle-{tag}-{}-{}.db",
        std::process::id(),
        now.as_nanos()
    ))
}

fn cleanup_db_path(path: PathBuf) {
    let _ = fs::remove_file(path);
}
