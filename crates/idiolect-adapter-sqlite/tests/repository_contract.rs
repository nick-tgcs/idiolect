use idiolect_adapter_sqlite::{SqliteMetadataStore, SqliteStorageErrorKind};
use idiolect_common::ids::ImeSessionId;
use idiolect_ports::storage::MetadataStorePort;

#[test]
fn migration_01_creates_event_log() {
    let mut store = SqliteMetadataStore::open_in_memory().expect("store should open");
    store.migrate().expect("migration should apply");

    assert!(store.table_exists_for_test("event_log").unwrap());
    assert_eq!(
        store.table_columns_for_test("event_log").unwrap(),
        [
            "id",
            "aggregate_type",
            "aggregate_id",
            "event_type",
            "event_version",
            "event_json",
            "idempotency_key",
            "created_at",
            "created_by",
        ]
    );
}

#[test]
fn migration_01_creates_materialized_tables() {
    let mut store = SqliteMetadataStore::open_in_memory().expect("store should open");
    store.migrate().expect("migration should apply");

    for table in [
        "schema_migrations",
        "ime_text_sessions",
        "ime_edit_events",
        "training_candidates",
        "adapters",
        "training_runs",
    ] {
        assert!(
            store.table_exists_for_test(table).unwrap(),
            "missing {table}"
        );
    }
    assert_eq!(
        store.applied_migration_versions_for_test().unwrap(),
        [1, 2, 3, 4, 5, 6]
    );
}

#[test]
fn migration_02_adds_correction_memory() {
    let mut store = SqliteMetadataStore::open_in_memory().expect("store should open");
    store.migrate().expect("migration should apply");

    assert!(store.table_exists_for_test("correction_memory").unwrap());
    assert_eq!(
        store.table_columns_for_test("correction_memory").unwrap(),
        [
            "id",
            "user_id",
            "raw_text",
            "corrected_text",
            "confidence",
            "occurrence_count",
            "first_seen_at",
            "last_seen_at",
        ]
    );
}

#[test]
fn migration_02_is_recorded_after_01() {
    let mut store = SqliteMetadataStore::open_in_memory().expect("store should open");
    store.migrate().expect("migration should apply");

    assert_eq!(
        store.applied_migration_versions_for_test().unwrap(),
        [1, 2, 3, 4, 5, 6]
    );
}

#[test]
fn migrate_is_idempotent() {
    let mut store = SqliteMetadataStore::open_in_memory().expect("store should open");
    store.migrate().expect("migration should apply");
    let first_rows = store.schema_migration_rows_for_test().unwrap();

    store.migrate().expect("second migration should be a no-op");

    assert_eq!(store.schema_migration_rows_for_test().unwrap(), first_rows);
}

#[test]
fn migrate_with_mismatched_checksum_fails_fast() {
    let mut store = SqliteMetadataStore::open_in_memory().expect("store should open");
    store.migrate().expect("migration should apply");
    store
        .force_schema_checksum_for_test(1, "not-the-real-checksum")
        .unwrap();

    let error = store.migrate().expect_err("checksum mismatch should fail");

    assert_eq!(
        error.kind(),
        SqliteStorageErrorKind::MigrationChecksumMismatch
    );
    assert!(error.to_string().contains("checksum mismatch"));
    assert_eq!(
        store.applied_migration_versions_for_test().unwrap(),
        [1, 2, 3, 4, 5, 6]
    );
}

#[test]
fn storage_errors_use_owned_error_kind() {
    let store = SqliteMetadataStore::open_in_memory().expect("store should open");

    let error = store
        .table_columns_for_test("bad table name)")
        .expect_err("invalid table name should fail");

    assert_eq!(error.kind(), SqliteStorageErrorKind::Backend);
}

#[test]
fn commit_session_is_idempotent_with_same_key() {
    let mut store = migrated_store();
    let store_port: &mut dyn MetadataStorePort<Error = _> = &mut store;

    let session_id = store_port
        .create_session(Some("restart traffic"))
        .expect("session should be created");
    store_port
        .commit_session(session_id, "restart Traefik", "commit-1")
        .expect("first commit should succeed");
    store_port
        .commit_session(session_id, "restart Traefik", "commit-1")
        .expect("duplicate commit should be idempotent");

    assert_event_count(&store, 2);
    assert_training_candidate_count(&store, 1);
    assert_session_state(&store, session_id, "committed");
}

#[test]
fn duplicate_idempotency_key_with_different_payload_is_conflict() {
    let mut store = migrated_store();
    let store_port: &mut dyn MetadataStorePort<Error = _> = &mut store;

    let session_id = store_port
        .create_session(Some("restart traffic"))
        .expect("session should be created");
    store_port
        .commit_session(session_id, "restart Traefik", "commit-1")
        .expect("first commit should succeed");

    let error = store_port
        .commit_session(session_id, "restart Traffic", "commit-1")
        .expect_err("conflicting payload should fail");

    assert_eq!(error.kind(), SqliteStorageErrorKind::IdempotencyConflict);
    assert_event_count(&store, 2);
    assert_training_candidate_count(&store, 1);
}

#[test]
fn cancel_session_after_commit_does_not_change_committed_row() {
    let mut store = migrated_store();
    let store_port: &mut dyn MetadataStorePort<Error = _> = &mut store;

    let session_id = store_port
        .create_session(Some("restart traffic"))
        .expect("session should be created");
    store_port
        .commit_session(session_id, "restart Traefik", "commit-1")
        .expect("commit should succeed");
    store_port
        .cancel_session(session_id, "cancel-1")
        .expect("cancel should be recorded");

    assert_session_state(&store, session_id, "committed");
    assert_event_count(&store, 3);
    assert_training_candidate_count(&store, 1);
}

#[test]
fn commit_session_without_created_session_fails_without_writes() {
    let mut store = migrated_store();
    let session_id = ImeSessionId::new();
    let store_port: &mut dyn MetadataStorePort<Error = _> = &mut store;

    let error = store_port
        .commit_session(session_id, "restart Traefik", "commit-unknown")
        .expect_err("unknown session commit should fail");

    assert_eq!(error.kind(), SqliteStorageErrorKind::Backend);
    assert_event_count(&store, 0);
    assert_training_candidate_count(&store, 0);
}

#[test]
fn audio_digest_persists_and_reads_back() {
    let mut store = migrated_store();
    let session_id = store
        .create_session(Some("restart traffic"))
        .expect("session should be created");
    let link = store
        .session_utterance_link_for_test(session_id)
        .expect("link should query")
        .expect("link should exist");

    // A freshly created utterance carries no audio digest yet — capture must
    // populate it, and nothing else does.
    assert_eq!(
        store
            .audio_digest_for_test(&link.utterance_id)
            .expect("digest should query"),
        None,
    );

    let digest = idiolect_common::digest::audio_sha256_hex(b"idopus1-payload-bytes");
    store
        .set_audio_digest(&link.utterance_id, &digest)
        .expect("digest should persist");

    assert_eq!(
        store
            .audio_digest_for_test(&link.utterance_id)
            .expect("digest should query"),
        Some(digest),
    );
}

#[test]
fn set_audio_digest_for_unknown_utterance_errors() {
    let store = migrated_store();
    let error = store
        .set_audio_digest("utterance:missing", "deadbeef")
        .expect_err("unknown utterance should fail");
    assert_eq!(error.kind(), SqliteStorageErrorKind::Backend);
}

fn migrated_store() -> SqliteMetadataStore {
    let mut store = SqliteMetadataStore::open_in_memory().expect("store should open");
    store.migrate().expect("migration should apply");
    store
}

fn assert_event_count(store: &SqliteMetadataStore, expected: i64) {
    assert_eq!(store.event_count_for_test().unwrap(), expected);
}

fn assert_training_candidate_count(store: &SqliteMetadataStore, expected: i64) {
    assert_eq!(store.training_candidate_count_for_test().unwrap(), expected);
}

fn assert_session_state(store: &SqliteMetadataStore, session_id: ImeSessionId, expected: &str) {
    assert_eq!(store.session_state_for_test(session_id).unwrap(), expected);
}
