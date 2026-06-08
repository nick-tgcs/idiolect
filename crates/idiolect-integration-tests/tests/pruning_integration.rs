//! Integration tests for background history pruning.

use std::time::Duration;

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_application::use_cases::maintenance::MaintenanceUseCase;
use idiolect_common::config::HistoryConfig;
use idiolect_ports::storage::MetadataStorePort;
use tempfile::tempdir;
use tokio::sync::watch;

/// Commits a transcript and backdates its history row to `created_at` so prune
/// behaviour can be exercised deterministically.
fn seed_entry(db_path: &str, text: &str, key: &str, created_at: &str) {
    let mut store = SqliteMetadataStore::open_path(db_path).unwrap();
    store.migrate().unwrap();
    let session = store.create_session(Some(text)).unwrap();
    store.commit_session(session, text, key).unwrap();

    let connection = rusqlite::Connection::open(db_path).unwrap();
    connection
        .execute(
            "UPDATE ime_text_history SET created_at = ?1 WHERE text = ?2",
            rusqlite::params![created_at, text],
        )
        .unwrap();
}

#[tokio::test]
async fn prune_removes_entries_older_than_retention_and_keeps_recent() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();

    seed_entry(db, "ancient", "old-key", "2000-01-01T00:00:00.000Z");
    // A clearly-recent entry that must survive a 1-day retention window.
    seed_entry(db, "fresh", "new-key", "2999-01-01T00:00:00.000Z");

    let store = SqliteMetadataStore::open_path(db).unwrap();
    let config = HistoryConfig {
        retention_days: 1,
        max_entries: 10,
        ..HistoryConfig::default()
    };
    let (_shutdown_tx, shutdown_rx) = watch::channel(());
    let mut maintenance = MaintenanceUseCase::new(store, config, shutdown_rx);

    let removed = maintenance.run_pruning_once().await.unwrap();
    assert_eq!(removed, 1);

    let store = SqliteMetadataStore::open_path(db).unwrap();
    let remaining = store.recent_history(10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].text, "fresh");
}

#[tokio::test]
async fn prune_disabled_with_zero_retention_is_a_noop() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();

    seed_entry(db, "ancient", "old-key", "2000-01-01T00:00:00.000Z");

    let store = SqliteMetadataStore::open_path(db).unwrap();
    let config = HistoryConfig {
        retention_days: 0,
        max_entries: 10,
        ..HistoryConfig::default()
    };
    let (_shutdown_tx, shutdown_rx) = watch::channel(());
    let mut maintenance = MaintenanceUseCase::new(store, config, shutdown_rx);

    let removed = maintenance.run_pruning_once().await.unwrap();
    assert_eq!(removed, 0);

    let store = SqliteMetadataStore::open_path(db).unwrap();
    assert_eq!(store.recent_history(10).unwrap().len(), 1);
}

#[tokio::test]
async fn pruning_loop_returns_ok_on_shutdown() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();

    let store = SqliteMetadataStore::open_path(db).unwrap();
    let mut store = store;
    store.migrate().unwrap();

    let config = HistoryConfig {
        retention_days: 1,
        max_entries: 10,
        ..HistoryConfig::default()
    };
    let (shutdown_tx, shutdown_rx) = watch::channel(());
    // Long interval so the only way the loop ends is via the shutdown signal.
    let maintenance = MaintenanceUseCase::new(store, config, shutdown_rx)
        .with_interval(Duration::from_secs(3600));

    let handle = tokio::spawn(maintenance.run_pruning_loop());
    shutdown_tx.send(()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("pruning loop should stop promptly after shutdown")
        .expect("pruning task should not panic");
    assert!(result.is_ok(), "clean shutdown must return Ok");
}

#[tokio::test]
async fn pruning_loop_disabled_retention_still_stops_on_shutdown() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();

    let mut store = SqliteMetadataStore::open_path(db).unwrap();
    store.migrate().unwrap();

    let config = HistoryConfig {
        retention_days: 0,
        max_entries: 10,
        ..HistoryConfig::default()
    };
    let (shutdown_tx, shutdown_rx) = watch::channel(());
    let maintenance = MaintenanceUseCase::new(store, config, shutdown_rx);

    let handle = tokio::spawn(maintenance.run_pruning_loop());
    shutdown_tx.send(()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("disabled pruning loop should still honour shutdown")
        .expect("pruning task should not panic");
    assert!(result.is_ok());
}
