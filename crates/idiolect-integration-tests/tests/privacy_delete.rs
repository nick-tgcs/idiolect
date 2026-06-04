use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ports::storage::MetadataStorePort;

#[test]
fn privacy_delete_removes_user_materialized_data_and_appends_event() {
    let path = unique_temp_db_path("privacy-delete");
    let mut store = SqliteMetadataStore::open_path(&path).expect("database should open");

    store.migrate().expect("store migration should succeed");

    let store_port: &mut dyn MetadataStorePort<
        Error = idiolect_adapter_sqlite::SqliteStorageError,
    > = &mut store;

    let session_id = store_port
        .create_session(Some("restart traffic"))
        .expect("session should be created");
    store_port
        .commit_session(session_id, "restart Traefik", "privacy-delete-commit-1")
        .expect("session commit should succeed");

    assert_eq!(
        store
            .training_candidate_count_for_test()
            .expect("candidate count should query"),
        1
    );

    store
        .delete_user_data_for_test("default")
        .expect("privacy deletion should succeed");

    assert_eq!(
        store
            .training_candidate_count_for_test()
            .expect("candidate count should query"),
        0
    );
    assert_eq!(
        store
            .user_data_deleted_event_count_for_test("default")
            .expect("deletion event count should query"),
        1
    );

    cleanup_db_path(path);
}

fn unique_temp_db_path(tag: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    env::temp_dir().join(format!(
        "idiolect-{tag}-{}-{}.db",
        std::process::id(),
        now.as_nanos()
    ))
}

fn cleanup_db_path(path: PathBuf) {
    let _ = fs::remove_file(path);
}
