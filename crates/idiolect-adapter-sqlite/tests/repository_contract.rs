use idiolect_adapter_sqlite::{SqliteMetadataStore, SqliteStorageErrorKind};

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
    assert_eq!(store.applied_migration_versions_for_test().unwrap(), [1, 2]);
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

    assert_eq!(store.applied_migration_versions_for_test().unwrap(), [1, 2]);
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
    assert_eq!(store.applied_migration_versions_for_test().unwrap(), [1, 2]);
}

#[test]
fn storage_errors_use_owned_error_kind() {
    let store = SqliteMetadataStore::open_in_memory().expect("store should open");

    let error = store
        .table_columns_for_test("bad table name)")
        .expect_err("invalid table name should fail");

    assert_eq!(error.kind(), SqliteStorageErrorKind::Backend);
}
