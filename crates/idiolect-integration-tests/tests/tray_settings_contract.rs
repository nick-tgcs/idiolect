use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ports::storage::MetadataStorePort;
use serde_json::Value;
use tempfile::tempdir;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[test]
fn tray_settings_get_set_roundtrip() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.sqlite");
    let mut store = SqliteMetadataStore::open_path(&db_path).unwrap();
    store.migrate().unwrap();

    // After migration, defaults are present
    let val = store.get_tray_setting("retention_days").unwrap();
    assert_eq!(val, Some("1".to_string()));

    // Set a value
    store.set_tray_setting("retention_days", "7").unwrap();

    // Get it back
    let val = store.get_tray_setting("retention_days").unwrap();
    assert_eq!(val, Some("7".to_string()));

    // Update it
    store.set_tray_setting("retention_days", "30").unwrap();
    let val = store.get_tray_setting("retention_days").unwrap();
    assert_eq!(val, Some("30".to_string()));
}

#[test]
fn tray_settings_get_all_returns_all() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.sqlite");
    let mut store = SqliteMetadataStore::open_path(&db_path).unwrap();
    store.migrate().unwrap();

    store.set_tray_setting("retention_days", "7").unwrap();
    store.set_tray_setting("max_entries", "25").unwrap();
    store
        .set_tray_setting("custom_key", "custom_value")
        .unwrap();

    let all = store.get_all_tray_settings().unwrap();
    assert_eq!(all.get("retention_days"), Some(&"7".to_string()));
    assert_eq!(all.get("max_entries"), Some(&"25".to_string()));
    assert_eq!(all.get("custom_key"), Some(&"custom_value".to_string()));
    assert_eq!(all.len(), 3);
}

#[test]
fn tray_settings_defaults_after_migration() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.sqlite");
    let mut store = SqliteMetadataStore::open_path(&db_path).unwrap();
    store.migrate().unwrap();

    // After migration, default values should be present
    let retention = store.get_tray_setting("retention_days").unwrap();
    let max_entries = store.get_tray_setting("max_entries").unwrap();

    // Defaults from HistoryConfig
    assert_eq!(retention, Some("1".to_string()));
    assert_eq!(max_entries, Some("10".to_string()));
}

#[test]
fn tray_settings_persist_across_reopen() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.sqlite");

    // First connection - set values
    {
        let mut store = SqliteMetadataStore::open_path(&db_path).unwrap();
        store.migrate().unwrap();
        store.set_tray_setting("retention_days", "30").unwrap();
        store.set_tray_setting("max_entries", "50").unwrap();
    }

    // Second connection - verify persistence
    {
        let mut store = SqliteMetadataStore::open_path(&db_path).unwrap();
        store.migrate().unwrap();
        let retention = store.get_tray_setting("retention_days").unwrap();
        let max_entries = store.get_tray_setting("max_entries").unwrap();
        assert_eq!(retention, Some("30".to_string()));
        assert_eq!(max_entries, Some("50".to_string()));
    }
}

#[test]
fn tray_status_cli_reports_defaults() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();

    let output = idiolect_cli::execute(&argv(&["tray", "status", "--db", db, "--json"])).unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["retention_days"], 1);
    assert_eq!(value["max_entries"], 10);
}

#[test]
fn tray_config_cli_persists_valid_values() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();

    let output = idiolect_cli::execute(&argv(&[
        "tray",
        "config",
        "--db",
        db,
        "--retention-days",
        "7",
        "--max-entries",
        "25",
        "--json",
    ]))
    .unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["retention_days"], 7);
    assert_eq!(value["max_entries"], 25);

    // Persisted in storage.
    let store = SqliteMetadataStore::open_path(db).unwrap();
    assert_eq!(
        store.get_tray_setting("retention_days").unwrap(),
        Some("7".to_string())
    );
}

#[test]
fn tray_config_cli_rejects_invalid_retention() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();

    let result = idiolect_cli::execute(&argv(&[
        "tray",
        "config",
        "--db",
        db,
        "--retention-days",
        "5",
    ]));
    assert!(result.is_err());

    // The rejected value must not have been written.
    let store = SqliteMetadataStore::open_path(db).unwrap();
    assert_eq!(
        store.get_tray_setting("retention_days").unwrap(),
        Some("1".to_string())
    );
}

#[test]
fn tray_menu_cli_dumps_menu_structure() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");
    let db = db.to_str().unwrap();

    let output = idiolect_cli::execute(&argv(&["tray", "menu", "--db", db, "--json"])).unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    let menu = value["menu"].as_array().unwrap();
    // Start/Stop/Cancel + separator + history + the Settings-window opener
    // (multi-choice settings live in the window, not in the menu).
    assert!(menu.iter().any(|item| item["id"] == "settings:open"));
    assert!(menu.iter().any(|item| item["id"] == "history"));
}
