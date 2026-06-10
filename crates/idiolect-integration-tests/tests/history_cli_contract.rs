//! Contract tests for the `idiolect-cli history ...` commands. These drive the
//! real `idiolect_cli::execute` entry point (not the store directly) so the
//! argument parsing, validation, and JSON output shape are all covered.

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ports::storage::MetadataStorePort;
use serde_json::Value;
use tempfile::tempdir;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

/// Seeds the database at `db_path` with the given committed transcripts and
/// returns the resulting history-entry ids (oldest first).
fn seed(db_path: &str, transcripts: &[&str]) -> Vec<i64> {
    let mut store = SqliteMetadataStore::open_path(db_path).unwrap();
    store.migrate().unwrap();
    for (index, text) in transcripts.iter().enumerate() {
        let session = store.create_session(Some(text)).unwrap();
        store
            .commit_session(session, text, &format!("seed-key-{index}"))
            .unwrap();
    }
    let mut entries = store.recent_history(100).unwrap();
    entries.sort_by_key(|entry| entry.id);
    entries.into_iter().map(|entry| entry.id).collect()
}

#[test]
fn history_list_json_returns_recent_entries_newest_first() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();
    seed(db, &["hello world", "foo bar"]);

    let output = idiolect_cli::execute(&argv(&["history", "list", "--db", db, "--json"]))
        .expect("history list should succeed");
    let value: Value = serde_json::from_str(&output).unwrap();
    let entries = value["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["text"], "foo bar");
    assert_eq!(entries[1]["text"], "hello world");
}

#[test]
fn history_list_respects_limit() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();
    seed(db, &["one", "two", "three"]);

    let output = idiolect_cli::execute(&argv(&[
        "history", "list", "--db", db, "--limit", "1", "--json",
    ]))
    .unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn history_show_returns_requested_entry() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();
    let ids = seed(db, &["alpha", "beta"]);
    let target = ids[0];

    let output = idiolect_cli::execute(&argv(&[
        "history",
        "show",
        "--id",
        &target.to_string(),
        "--db",
        db,
        "--json",
    ]))
    .unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["id"], target);
    assert_eq!(value["text"], "alpha");
}

#[test]
fn history_show_missing_entry_is_error() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();
    seed(db, &["alpha"]);

    let result = idiolect_cli::execute(&argv(&[
        "history", "show", "--id", "9999", "--db", db, "--json",
    ]));
    assert!(result.is_err());
}

#[test]
fn history_delete_requires_confirmation() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();
    let ids = seed(db, &["alpha"]);
    let id = ids[0].to_string();

    // Without --confirm-delete: refused, entry remains.
    let refused = idiolect_cli::execute(&argv(&["history", "delete", "--id", &id, "--db", db]));
    assert!(refused.is_err());

    let store = SqliteMetadataStore::open_path(db).unwrap();
    assert_eq!(store.recent_history(10).unwrap().len(), 1);
}

#[test]
fn history_delete_with_confirmation_removes_entry() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();
    let ids = seed(db, &["alpha", "beta"]);
    let id = ids[0].to_string();

    let output = idiolect_cli::execute(&argv(&[
        "history",
        "delete",
        "--id",
        &id,
        "--db",
        db,
        "--confirm-delete",
    ]))
    .unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["deleted"], true);

    let store = SqliteMetadataStore::open_path(db).unwrap();
    let remaining = store.recent_history(10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining.iter().all(|entry| entry.id != ids[0]));
}

#[test]
fn history_prune_requires_confirmation() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();
    seed(db, &["alpha"]);

    let refused = idiolect_cli::execute(&argv(&["history", "prune", "--days", "30", "--db", db]));
    assert!(refused.is_err());
}

#[test]
fn history_prune_reports_deleted_count() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();
    let ids = seed(db, &["alpha"]);

    // Backdate the entry well beyond the retention window.
    let connection = rusqlite::Connection::open(db).unwrap();
    connection
        .execute(
            "UPDATE ime_text_history SET created_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
            [ids[0]],
        )
        .unwrap();
    drop(connection);

    let output = idiolect_cli::execute(&argv(&[
        "history",
        "prune",
        "--days",
        "1",
        "--db",
        db,
        "--confirm-delete",
    ]))
    .unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["deleted_count"], 1);

    let store = SqliteMetadataStore::open_path(db).unwrap();
    assert_eq!(store.recent_history(10).unwrap().len(), 0);
}

#[test]
fn history_reinsert_without_daemon_is_connection_error() {
    let result = idiolect_cli::execute(&argv(&[
        "history",
        "reinsert",
        "--id",
        "1",
        "--socket",
        "/nonexistent/idiolect-test.sock",
    ]));
    assert!(result.is_err());
}

#[test]
fn history_copy_without_daemon_is_connection_error() {
    let result = idiolect_cli::execute(&argv(&[
        "history",
        "copy",
        "--id",
        "1",
        "--socket",
        "/nonexistent/idiolect-test.sock",
    ]));
    assert!(result.is_err());
}

#[test]
fn history_unknown_action_is_usage_error() {
    let result = idiolect_cli::execute(&argv(&["history", "bogus"]));
    assert!(result.is_err());
}
