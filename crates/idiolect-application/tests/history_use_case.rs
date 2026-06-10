//! Behavioural contract for `HistoryUseCase` — the application-layer
//! orchestration behind the history tray actions (reinsert / copy / delete /
//! prune). Driven entirely through the in-memory fakes so every branch,
//! including the NotFound paths, is exercised without real I/O.

use idiolect_application::use_cases::history::{
    ClipboardPort, HistoryUseCase, HistoryUseCaseError,
};
use idiolect_ports::storage::MetadataStorePort;
use idiolect_test_support::fakes::{FakeInputMethod, FakeMetadataStore};

/// Minimal clipboard fake: records the most recently set text.
#[derive(Default)]
struct FakeClipboard {
    last: Option<String>,
}

impl ClipboardPort for FakeClipboard {
    type Error = std::convert::Infallible;
    fn set_text(&mut self, text: &str) -> Result<(), Self::Error> {
        self.last = Some(text.to_owned());
        Ok(())
    }
}

/// Seed a store with two committed history rows and return it.
fn seeded_store() -> FakeMetadataStore {
    let mut store = FakeMetadataStore::default();
    let s1 = store.create_session(None).expect("session 1");
    store
        .commit_session(s1, "first entry", "k1")
        .expect("commit 1");
    let s2 = store.create_session(None).expect("session 2");
    store
        .commit_session(s2, "second entry", "k2")
        .expect("commit 2");
    store
}

fn use_case() -> HistoryUseCase<FakeInputMethod, FakeMetadataStore, FakeClipboard> {
    HistoryUseCase::new(
        FakeInputMethod::default(),
        seeded_store(),
        FakeClipboard::default(),
    )
}

#[test]
fn get_recent_returns_committed_entries_newest_first() {
    let uc = use_case();
    let recent = uc.get_recent(10).expect("recent history");
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].text, "second entry");
}

#[test]
fn reinsert_commits_a_present_entry_and_404s_for_an_absent_one() {
    let mut uc = use_case();
    let id = uc.get_recent(10).expect("recent")[0].id;
    uc.reinsert(id).expect("reinsert should commit the entry");
    assert!(matches!(
        uc.reinsert(987_654),
        Err(HistoryUseCaseError::NotFound)
    ));
}

#[test]
fn copy_writes_a_present_entry_and_404s_for_an_absent_one() {
    let mut uc = use_case();
    let id = uc.get_recent(10).expect("recent")[0].id;
    uc.copy(id).expect("copy should set the clipboard");
    assert!(matches!(
        uc.copy(987_654),
        Err(HistoryUseCaseError::NotFound)
    ));
}

#[test]
fn delete_removes_a_single_entry() {
    let mut uc = use_case();
    let id = uc.get_recent(10).expect("recent")[0].id;
    uc.delete(id).expect("delete should succeed");
    let remaining = uc.get_recent(10).expect("recent");
    assert_eq!(remaining.len(), 1);
    assert!(remaining.iter().all(|e| e.id != id));
}

#[test]
fn prune_with_zero_days_clears_all_history() {
    let mut uc = use_case();
    let pruned = uc.prune(0).expect("prune should succeed");
    assert_eq!(pruned, 2);
    assert!(uc.get_recent(10).expect("recent").is_empty());
}
