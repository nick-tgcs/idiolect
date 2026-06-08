//! Contract tests for domain events emitted by history/tray mutations.
//!
//! These events are appended to the existing `event_log` (the same log the
//! session lifecycle already uses), keeping the store's audit trail complete.

use std::path::Path;

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ports::storage::MetadataStorePort;
use rusqlite::Connection;
use tempfile::tempdir;

fn count_events(db: &Path, event_type: &str) -> i64 {
    let connection = Connection::open(db).unwrap();
    connection
        .query_row(
            "SELECT COUNT(*) FROM event_log WHERE event_type = ?1",
            [event_type],
            |row| row.get(0),
        )
        .unwrap()
}

fn commit(store: &mut SqliteMetadataStore, text: &str, key: &str) -> i64 {
    let session = store.create_session(Some(text)).unwrap();
    store.commit_session(session, text, key).unwrap();
    store
        .recent_history(1)
        .unwrap()
        .first()
        .expect("history entry materialized")
        .id
}

#[test]
fn committing_a_session_still_appends_to_the_event_log() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let mut store = SqliteMetadataStore::open_path(&db).unwrap();
    store.migrate().unwrap();
    commit(&mut store, "hello", "k1");
    assert_eq!(count_events(&db, "SessionCommitted"), 1);
}

#[test]
fn deleting_history_emits_a_domain_event() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let mut store = SqliteMetadataStore::open_path(&db).unwrap();
    store.migrate().unwrap();
    let id = commit(&mut store, "hello", "k1");

    store.delete_history_entry(id).unwrap();
    assert_eq!(count_events(&db, "HistoryEntryDeleted"), 1);
}

#[test]
fn pruning_emits_an_event_only_when_rows_are_removed() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let mut store = SqliteMetadataStore::open_path(&db).unwrap();
    store.migrate().unwrap();
    commit(&mut store, "recent", "k1");

    // Nothing old enough to prune: no event.
    assert_eq!(store.prune_history(30).unwrap(), 0);
    assert_eq!(count_events(&db, "HistoryPruned"), 0);

    // Backdate the row and prune: exactly one event.
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE ime_text_history SET created_at = '2000-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
    assert_eq!(store.prune_history(1).unwrap(), 1);
    assert_eq!(count_events(&db, "HistoryPruned"), 1);
}

#[test]
fn tray_config_changes_emit_events_with_unique_keys() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let mut store = SqliteMetadataStore::open_path(&db).unwrap();
    store.migrate().unwrap();

    // Migration defaults are seeded without events.
    assert_eq!(count_events(&db, "TrayConfigChanged"), 0);

    store.set_tray_setting("retention_days", "7").unwrap();
    store.set_tray_setting("retention_days", "30").unwrap();
    assert_eq!(count_events(&db, "TrayConfigChanged"), 2);
}
