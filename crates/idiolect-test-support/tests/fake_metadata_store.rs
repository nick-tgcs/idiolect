//! Behavioural contract for `FakeMetadataStore` — the in-memory store many unit
//! tests rely on. Because so many tests trust it as a stand-in for the real
//! SQLite store, its history/idempotency/pruning behaviour is itself worth
//! pinning so a regression here can't quietly corrupt the suites that use it.

use idiolect_ports::storage::{HistoryState, MetadataStorePort};
use idiolect_test_support::fakes::{FakeInputMethod, FakeMetadataStore};

use idiolect_common::ids::ImeSessionId;
use idiolect_ports::input_method::InputMethodPort;

#[test]
fn commit_is_idempotent_and_recorded_in_history() {
    let mut store = FakeMetadataStore::default();
    let session = store
        .create_session(Some("restart traffic"))
        .expect("session creates");
    store
        .record_preedit_change(session, "restart traffic", "restart Traefik", 0)
        .expect("preedit change records");
    store
        .commit_session(session, "restart Traefik", "key-1")
        .expect("commit succeeds");
    // Same idempotency key must not double-count or duplicate history.
    store
        .commit_session(session, "restart Traefik", "key-1")
        .expect("repeat commit is a no-op");

    assert_eq!(store.training_candidate_count(), 1);
    let history = store.recent_history(10).expect("recent history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text, "restart Traefik");
    assert_eq!(history[0].state, HistoryState::Committed);
    assert!(store.events().contains(&"commit:restart Traefik"));
}

#[test]
fn cancel_records_an_empty_committed_history_row() {
    let mut store = FakeMetadataStore::default();
    let session = store.create_session(None).expect("session creates");
    store
        .cancel_session(session, "cancel-1")
        .expect("cancel succeeds");

    let entry = store
        .history_entries()
        .into_iter()
        .next()
        .expect("a cancelled row exists");
    assert_eq!(entry.state, HistoryState::Cancelled);
    assert!(entry.text.is_empty());
    assert_eq!(store.training_candidate_count(), 0);
}

#[test]
fn get_history_entry_finds_present_and_returns_none_for_absent() {
    let mut store = FakeMetadataStore::default();
    let session = store.create_session(None).expect("session creates");
    store
        .commit_session(session, "hello", "k")
        .expect("commit succeeds");
    let id = store.recent_history(1).expect("history")[0].id;

    assert!(store.get_history_entry(id).expect("lookup").is_some());
    assert!(store
        .get_history_entry(id + 9999)
        .expect("lookup")
        .is_none());
}

#[test]
fn delete_removes_a_single_entry() {
    let mut store = FakeMetadataStore::default();
    let session = store.create_session(None).expect("session creates");
    store.commit_session(session, "a", "k1").expect("commit a");
    store.commit_session(session, "b", "k2").expect("commit b");
    let id = store.recent_history(10).expect("history")[0].id;

    store.delete_history_entry(id).expect("delete succeeds");
    let remaining = store.recent_history(10).expect("history");
    assert_eq!(remaining.len(), 1);
    assert!(remaining.iter().all(|e| e.id != id));
}

#[test]
fn prune_history_with_zero_days_clears_everything() {
    let mut store = FakeMetadataStore::default();
    let session = store.create_session(None).expect("session creates");
    store.commit_session(session, "old", "k").expect("commit");
    // A cutoff of "now" prunes rows created before this instant.
    let pruned = store.prune_history(0).expect("prune succeeds");
    assert_eq!(pruned, 1);
    assert!(store.recent_history(10).expect("history").is_empty());
}

#[test]
fn tray_settings_round_trip_individually_and_in_bulk() {
    let mut store = FakeMetadataStore::default();
    assert!(store.get_tray_setting("theme").expect("get").is_none());
    store.set_tray_setting("theme", "dark").expect("set theme");
    store.set_tray_setting("lang", "en").expect("set lang");

    assert_eq!(
        store
            .get_tray_setting("theme")
            .expect("get theme")
            .as_deref(),
        Some("dark")
    );
    let all = store.get_all_tray_settings().expect("all settings");
    assert_eq!(all.len(), 2);
    assert_eq!(all.get("lang").map(String::as_str), Some("en"));
}

#[test]
fn fake_input_method_logs_the_full_preedit_lifecycle() {
    let mut input = FakeInputMethod::default();
    let session = ImeSessionId::new();
    input.show_preedit(session, "a").expect("show");
    input.update_preedit(session, "ab").expect("update");
    input.cancel_preedit(session).expect("cancel");
    assert_eq!(
        input.events(),
        ["show_preedit:a", "update_preedit:ab", "cancel_preedit"]
    );
}
