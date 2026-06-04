use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_sqlite::migrations::migration_by_version;
use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ports::storage::MetadataStorePort;
use rusqlite::{params, Connection};

#[test]
fn v1_schema_has_users_utterances_audio_and_session_links() {
    let store = migrated_store();

    for table in [
        "users",
        "utterances",
        "utterance_audio_files",
        "ime_text_sessions",
        "ime_edit_events",
        "training_candidates",
        "manifests",
        "manifest_items",
        "adapter_derivations",
        "retention_tombstones",
        "correction_memory",
    ] {
        assert!(
            store.table_exists_for_test(table).unwrap(),
            "missing {table}"
        );
    }

    assert_columns_include(
        &store,
        "utterances",
        &[
            "id",
            "user_id",
            "audio_path",
            "audio_codec",
            "audio_container",
            "sample_rate_hz",
            "duration_ms",
            "raw_stt_text",
            "stt_model",
            "language",
        ],
    );
    assert_columns_include(
        &store,
        "ime_text_sessions",
        &[
            "utterance_id",
            "user_id",
            "platform",
            "input_backend",
            "session_state",
            "initial_preedit_text",
            "final_preedit_text",
            "edit_capture_quality",
        ],
    );
    assert_columns_include(
        &store,
        "training_candidates",
        &[
            "utterance_id",
            "text_session_id",
            "candidate_transcript",
            "status",
            "classifier_label",
        ],
    );
    assert_columns_include(&store, "correction_memory", &["user_id"]);
    assert_eq!(
        store.applied_migration_versions_for_test().unwrap(),
        [1, 2, 3]
    );
}

#[test]
fn v1_schema_declares_foreign_keys_for_new_relationship_tables() {
    let store = migrated_store();

    assert_foreign_key(&store, "utterances", "user_id", "users", "id");
    assert_foreign_key(
        &store,
        "ime_text_sessions",
        "utterance_id",
        "utterances",
        "id",
    );
    assert_foreign_key(&store, "ime_text_sessions", "user_id", "users", "id");
    assert_foreign_key(
        &store,
        "ime_edit_events",
        "text_session_id",
        "ime_text_sessions",
        "id",
    );
    assert_foreign_key(
        &store,
        "training_candidates",
        "utterance_id",
        "utterances",
        "id",
    );
    assert_foreign_key(
        &store,
        "training_candidates",
        "text_session_id",
        "ime_text_sessions",
        "id",
    );
    assert_foreign_key(
        &store,
        "utterance_audio_files",
        "utterance_id",
        "utterances",
        "id",
    );
    assert_foreign_key(&store, "manifests", "user_id", "users", "id");
    assert_foreign_key(&store, "manifest_items", "manifest_id", "manifests", "id");
    assert_foreign_key(
        &store,
        "manifest_items",
        "training_candidate_id",
        "training_candidates",
        "id",
    );
    assert_foreign_key(&store, "adapter_derivations", "user_id", "users", "id");
    assert_foreign_key(&store, "retention_tombstones", "user_id", "users", "id");
}

#[test]
fn sqlite_store_enforces_foreign_keys_on_runtime_writes() {
    let store = migrated_store();

    let result = store.insert_dangling_training_candidate_for_test();

    assert!(
        result.is_err(),
        "dangling training candidate should be rejected by SQLite foreign keys"
    );
}

#[test]
fn migration_03_upgrades_populated_v2_rows_with_utterance_links() {
    let path = unique_temp_db_path("populated-v2");
    {
        let connection = Connection::open(&path).expect("v2 database should open");
        install_migration_for_test(&connection, 1);
        install_migration_for_test(&connection, 2);
        connection
            .execute(
                "INSERT INTO ime_text_sessions(id, raw_stt_text, state)
                 VALUES ('legacy-session', 'restart traffic', 'created')",
                [],
            )
            .expect("legacy session should insert");
        connection
            .execute(
                "INSERT INTO ime_edit_events(session_id, from_text, to_text, event_index)
                 VALUES ('legacy-session', 'restart traffic', 'restart Traefik', 1)",
                [],
            )
            .expect("legacy edit should insert");
        connection
            .execute(
                "INSERT INTO training_candidates(
                    session_id,
                    raw_text,
                    corrected_text,
                    source,
                    trust_score,
                    capture_quality,
                    idempotency_key
                 ) VALUES (
                    'legacy-session',
                    'restart traffic',
                    'restart Traefik',
                    'accepted_without_edit',
                    1.0,
                    'live',
                    'legacy-candidate'
                 )",
                [],
            )
            .expect("legacy candidate should insert");
    }

    let mut store = SqliteMetadataStore::open_path(&path).expect("store should reopen");
    store.migrate().expect("v3 migration should apply");

    let connection = Connection::open(&path).expect("upgraded database should open");
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*)
             FROM ime_text_sessions AS s
             JOIN utterances AS u ON u.id = s.utterance_id
             JOIN utterance_audio_files AS a ON a.utterance_id = u.id
             WHERE s.id = 'legacy-session'",
        ),
        1
    );
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*)
             FROM training_candidates AS tc
             JOIN ime_text_sessions AS s ON s.id = tc.text_session_id
             JOIN utterances AS u ON u.id = tc.utterance_id
             WHERE tc.id = 1",
        ),
        1
    );

    cleanup_db_path(path);
}

#[test]
fn migration_03_backfills_legacy_training_and_adapter_users() {
    let path = unique_temp_db_path("legacy-users");
    {
        let connection = Connection::open(&path).expect("v2 database should open");
        install_migration_for_test(&connection, 1);
        install_migration_for_test(&connection, 2);
        connection
            .execute(
                "INSERT INTO training_runs(id, user_id, manifest_digest, status)
                 VALUES ('run-1', 'run-user', 'manifest', 'complete')",
                [],
            )
            .expect("legacy training run should insert");
        connection
            .execute(
                "INSERT INTO adapters(
                    id,
                    user_id,
                    artifact_digest,
                    manifest_digest,
                    metric_report_digest,
                    active
                 ) VALUES (
                    'adapter-1',
                    'adapter-user',
                    'artifact',
                    'manifest',
                    'metrics',
                    0
                 )",
                [],
            )
            .expect("legacy adapter should insert");
    }

    let mut store = SqliteMetadataStore::open_path(&path).expect("store should reopen");
    store.migrate().expect("v3 migration should apply");

    let connection = Connection::open(&path).expect("upgraded database should open");
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM users WHERE id IN ('run-user', 'adapter-user')",
        ),
        2
    );
    assert_eq!(foreign_key_violation_count(&connection), 0);

    cleanup_db_path(path);
}

#[test]
fn privacy_export_counts_only_requested_user_candidates() {
    let path = unique_temp_db_path("privacy-export-user");
    {
        let mut store = SqliteMetadataStore::open_path(&path).expect("store should open");
        store.migrate().expect("migration should apply");
    }
    {
        let connection = Connection::open(&path).expect("database should open");
        insert_candidate_for_user(&connection, "default", "one");
        insert_candidate_for_user(&connection, "other", "two");
    }

    let store = SqliteMetadataStore::open_path(&path).expect("store should reopen");
    let summary = store
        .privacy_export_summary("default")
        .expect("privacy export summary should query");

    assert_eq!(summary.training_candidates, 1);

    cleanup_db_path(path);
}

#[test]
fn privacy_export_counts_only_requested_user_correction_memory() {
    let path = unique_temp_db_path("privacy-export-correction-memory");
    {
        let mut store = SqliteMetadataStore::open_path(&path).expect("store should open");
        store.migrate().expect("migration should apply");
    }
    {
        let connection = Connection::open(&path).expect("database should open");
        connection
            .execute(
                "INSERT OR IGNORE INTO users(id, display_name) VALUES ('other', 'other')",
                [],
            )
            .expect("other user should insert");
        connection
            .execute(
                "INSERT INTO correction_memory(
                    user_id,
                    raw_text,
                    corrected_text,
                    confidence,
                    occurrence_count
                 ) VALUES (?1, ?2, ?3, 1.0, 1)",
                params!["default", "restart traffic", "restart Traefik"],
            )
            .expect("default correction should insert");
        connection
            .execute(
                "INSERT INTO correction_memory(
                    user_id,
                    raw_text,
                    corrected_text,
                    confidence,
                    occurrence_count
                 ) VALUES (?1, ?2, ?3, 1.0, 1)",
                params!["other", "open browser", "open Browser"],
            )
            .expect("other correction should insert");
    }

    let store = SqliteMetadataStore::open_path(&path).expect("store should reopen");
    let summary = store
        .privacy_export_summary("default")
        .expect("privacy export summary should query");

    assert_eq!(summary.correction_memory_entries, 1);

    cleanup_db_path(path);
}

#[test]
fn delete_user_data_keeps_other_users_correction_memory() {
    let path = unique_temp_db_path("correction-memory-user");
    {
        let mut store = SqliteMetadataStore::open_path(&path).expect("store should open");
        store.migrate().expect("migration should apply");
    }
    {
        let connection = Connection::open(&path).expect("database should open");
        connection
            .execute(
                "INSERT OR IGNORE INTO users(id, display_name) VALUES ('other', 'other')",
                [],
            )
            .expect("other user should insert");
        connection
            .execute(
                "INSERT INTO correction_memory(
                    user_id,
                    raw_text,
                    corrected_text,
                    confidence,
                    occurrence_count
                 ) VALUES (?1, ?2, ?3, 1.0, 1)",
                params!["default", "restart traffic", "restart Traefik"],
            )
            .expect("default correction should insert");
        connection
            .execute(
                "INSERT INTO correction_memory(
                    user_id,
                    raw_text,
                    corrected_text,
                    confidence,
                    occurrence_count
                 ) VALUES (?1, ?2, ?3, 1.0, 1)",
                params!["other", "open browser", "open Browser"],
            )
            .expect("other correction should insert");
    }

    let mut store = SqliteMetadataStore::open_path(&path).expect("store should reopen");
    store
        .delete_user_data_for_test("default")
        .expect("default user delete should succeed");

    let connection = Connection::open(&path).expect("database should reopen");
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM correction_memory WHERE user_id = 'default'",
        ),
        0
    );
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM correction_memory WHERE user_id = 'other'",
        ),
        1
    );

    cleanup_db_path(path);
}

#[test]
fn committed_session_links_exactly_one_utterance() {
    let mut store = migrated_store();
    let session_id = {
        let store_port: &mut dyn MetadataStorePort<Error = _> = &mut store;
        let session_id = store_port
            .create_session(Some("restart traffic"))
            .expect("session should be created");
        store_port
            .commit_session(session_id, "restart Traefik", "commit-v1-link")
            .expect("session should commit");
        session_id
    };

    let link = store
        .session_utterance_link_for_test(session_id)
        .expect("session link should query")
        .expect("committed session should link an utterance");

    assert_eq!(link.user_id, "default");
    assert_eq!(link.session_state, "committed");
    assert!(!link.utterance_id.is_empty());
}

#[test]
fn training_candidate_links_session_and_utterance() {
    let mut store = migrated_store();
    {
        let store_port: &mut dyn MetadataStorePort<Error = _> = &mut store;
        let session_id = store_port
            .create_session(Some("restart traffic"))
            .expect("session should be created");
        store_port
            .commit_session(session_id, "restart Traefik", "commit-v1-candidate")
            .expect("session should commit");
    }

    let links = store
        .training_candidate_links_for_test()
        .expect("candidate links should query");

    assert_eq!(links.len(), 1);
    assert_eq!(links[0].status, "captured");
    assert_eq!(links[0].text_session_count, 1);
    assert_eq!(links[0].utterance_count, 1);
}

#[test]
fn delete_user_keeps_tombstone_but_removes_private_rows() {
    let mut store = migrated_store();
    {
        let store_port: &mut dyn MetadataStorePort<Error = _> = &mut store;
        let session_id = store_port
            .create_session(Some("restart traffic"))
            .expect("session should be created");
        store_port
            .record_preedit_change(session_id, "restart traffic", "restart Traefik", 1)
            .expect("edit event should record");
        store_port
            .commit_session(session_id, "restart Traefik", "commit-v1-delete")
            .expect("session should commit");
    }

    store
        .delete_user_data_for_test("default")
        .expect("privacy delete should succeed");

    let counts = store
        .private_row_counts_for_test("default")
        .expect("private row counts should query");
    assert_eq!(counts.utterances, 0);
    assert_eq!(counts.text_sessions, 0);
    assert_eq!(counts.edit_events, 0);
    assert_eq!(counts.training_candidates, 0);
    assert_eq!(counts.manifest_items, 0);
    assert_eq!(counts.tombstones, 1);
}

fn install_migration_for_test(connection: &Connection, version: i64) {
    let migration = migration_by_version(version).expect("migration should exist");
    connection
        .execute_batch(migration.sql)
        .expect("migration sql should apply");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, name, checksum) VALUES (?1, ?2, ?3)",
            params![
                migration.version,
                migration.name,
                migration.expected_sha256_hex
            ],
        )
        .expect("migration row should insert");
}

fn insert_candidate_for_user(connection: &Connection, user_id: &str, suffix: &str) {
    connection
        .execute(
            "INSERT OR IGNORE INTO users(id, display_name) VALUES (?1, ?1)",
            [user_id],
        )
        .expect("user should insert");
    let utterance_id = format!("utterance-{suffix}");
    let session_id = format!("session-{suffix}");
    connection
        .execute(
            "INSERT INTO utterances(
                id,
                user_id,
                audio_path,
                audio_codec,
                audio_container,
                sample_rate_hz,
                training_sample_rate_hz,
                channels,
                duration_ms,
                raw_stt_text,
                stt_model,
                language
             ) VALUES (?1, ?2, ?3, 'opus', 'ogg', 16000, 16000, 1, 0, 'raw', 'fixture', 'en')",
            params![
                utterance_id,
                user_id,
                format!("audio/2026/06/03/{suffix}.ogg")
            ],
        )
        .expect("utterance should insert");
    connection
        .execute(
            "INSERT INTO ime_text_sessions(
                id,
                raw_stt_text,
                committed_text,
                state,
                utterance_id,
                user_id,
                platform,
                input_backend,
                session_state,
                edit_capture_quality,
                started_at
             ) VALUES (?1, 'raw', 'corrected', 'committed', ?2, ?3, 'linux', 'fcitx5', 'committed', 'live', '2026-06-03T00:00:00Z')",
            params![session_id, utterance_id, user_id],
        )
        .expect("session should insert");
    connection
        .execute(
            "INSERT INTO training_candidates(
                session_id,
                raw_text,
                corrected_text,
                source,
                trust_score,
                capture_quality,
                idempotency_key,
                utterance_id,
                text_session_id,
                candidate_transcript,
                status
             ) VALUES (?1, 'raw', 'corrected', 'accepted_without_edit', 1.0, 'live', ?2, ?3, ?1, 'corrected', 'captured')",
            params![session_id, format!("candidate-{suffix}"), utterance_id],
        )
        .expect("candidate should insert");
}

fn scalar_count(connection: &Connection, sql: &str) -> i64 {
    connection
        .query_row(sql, [], |row| row.get(0))
        .expect("count should query")
}

fn foreign_key_violation_count(connection: &Connection) -> i64 {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .expect("foreign key check should prepare");
    let rows = statement
        .query_map([], |_| Ok(()))
        .expect("foreign key check should query");
    rows.count()
        .try_into()
        .expect("foreign key check count should fit")
}

fn unique_temp_db_path(tag: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    env::temp_dir().join(format!(
        "idiolect-v1-schema-{tag}-{}-{}.db",
        std::process::id(),
        now.as_nanos()
    ))
}

fn cleanup_db_path(path: PathBuf) {
    let _ = fs::remove_file(path);
}

fn assert_foreign_key(
    store: &SqliteMetadataStore,
    table: &str,
    from_column: &str,
    to_table: &str,
    to_column: &str,
) {
    let foreign_keys = store.foreign_keys_for_test(table).unwrap();
    assert!(
        foreign_keys.iter().any(|foreign_key| {
            foreign_key.from_column == from_column
                && foreign_key.table == to_table
                && foreign_key.to_column == to_column
        }),
        "{table} missing foreign key {from_column} -> {to_table}({to_column}); got {foreign_keys:?}"
    );
}

fn migrated_store() -> SqliteMetadataStore {
    let mut store = SqliteMetadataStore::open_in_memory().expect("store should open");
    store.migrate().expect("migration should apply");
    store
}

fn assert_columns_include(store: &SqliteMetadataStore, table: &str, expected: &[&str]) {
    let columns = store.table_columns_for_test(table).unwrap();
    for column in expected {
        assert!(
            columns.iter().any(|actual| actual == column),
            "{table} missing column {column}; columns: {columns:?}"
        );
    }
}
