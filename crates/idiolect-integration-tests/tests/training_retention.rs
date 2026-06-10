//! Integration test for training-data retention: `prune_training_data` purges
//! audio + transcript + training candidate + session for everything older than
//! the window, and leaves recent data — and audio of all kinds — untouched.
//!
//! Mirrors the privacy-retention/pruning integration tests: a real SQLite store,
//! a real `FileAudioStore` writing real files, with timestamps backdated via a
//! direct connection (the same trick `pruning_integration.rs` uses).

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_common::ids::ImeSessionId;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioObjectRef, AudioStorePort, MetadataStorePort};
use idiolect_test_support::fixtures::restart_traffic_fixture_16khz_mono;

#[test]
fn prune_purges_sessions_older_than_the_window_and_keeps_recent_ones() {
    let fixture = Fixture::new("prune-old");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    // An old, expired session and a recent one — each with real audio on disk.
    let (old_session, old_audio) = seed_session(&mut store, &audio_store, "restart traffic");
    let (_new_session, new_audio) = seed_session(&mut store, &audio_store, "deploy traefik");
    backdate_committed(&fixture.db_path(), old_session, "-400 days");

    assert_eq!(store.training_candidate_count_for_test().expect("count"), 2);

    // Keep a year: the 400-day-old session is purged, the fresh one survives.
    let purged = store
        .prune_training_data(365, &audio_store)
        .expect("prune should run");

    assert_eq!(purged, 1, "exactly the expired session is purged");
    assert_eq!(
        store.training_candidate_count_for_test().expect("count"),
        1,
        "the recent session's candidate remains"
    );
    assert!(
        !audio_store.source_audio_exists_for_test(&old_audio).expect("query"),
        "expired session's audio is deleted"
    );
    assert!(
        audio_store.source_audio_exists_for_test(&new_audio).expect("query"),
        "recent session's audio is kept (all audio is training data)"
    );
    let surviving: Vec<String> = store
        .recent_history(50)
        .expect("recent history")
        .into_iter()
        .map(|entry| entry.text)
        .collect();
    assert_eq!(surviving, vec!["deploy traefik".to_owned()], "only the recent transcript remains");
}

#[test]
fn prune_with_zero_retention_is_disabled() {
    let fixture = Fixture::new("prune-disabled");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    let (old_session, old_audio) = seed_session(&mut store, &audio_store, "restart traffic");
    backdate_committed(&fixture.db_path(), old_session, "-4000 days");

    let purged = store
        .prune_training_data(0, &audio_store)
        .expect("prune should run");

    assert_eq!(purged, 0, "retention 0 disables pruning");
    assert_eq!(store.training_candidate_count_for_test().expect("count"), 1);
    assert!(
        audio_store.source_audio_exists_for_test(&old_audio).expect("query"),
        "nothing is deleted when retention is disabled"
    );
}

/// Commit a session with real audio on disk; returns its id and the audio ref.
fn seed_session(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    text: &str,
) -> (ImeSessionId, AudioObjectRef) {
    let session_id = store.create_session(Some(text)).expect("create session");
    store
        .commit_session(session_id, text, &format!("commit-{text}"))
        .expect("commit session");
    let utterance_id = store
        .session_utterance_link_for_test(session_id)
        .expect("session link should query")
        .expect("session should have utterance")
        .utterance_id;

    let segment = restart_traffic_fixture_16khz_mono();
    let encoded = OpusCodec::new().encode(&segment).expect("encode fixture");
    let audio_ref = audio_store
        .write_source_audio("default", &utterance_id, &encoded)
        .expect("write source audio");
    (session_id, audio_ref)
}

/// Backdate a committed session's `committed_at` via a direct connection so the
/// retention window treats it as expired (same approach as pruning_integration).
fn backdate_committed(db_path: &PathBuf, session_id: ImeSessionId, offset: &str) {
    // `session_key` (the row id) is the JSON-serialized id *with* its quotes —
    // do not trim them, or the UPDATE matches nothing.
    let session_key = serde_json::to_string(&session_id).expect("serialize session id");
    let conn = rusqlite::Connection::open(db_path).expect("open db");
    conn.execute(
        "UPDATE ime_text_sessions
         SET committed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)
         WHERE id = ?2",
        rusqlite::params![offset, session_key],
    )
    .expect("backdate committed_at");
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
        let root = env::temp_dir().join(format!(
            "idiolect-training-retention-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root");
        Self { root }
    }

    fn db_path(&self) -> PathBuf {
        self.root.join("idiolect.sqlite")
    }

    fn open_store(&self) -> SqliteMetadataStore {
        let mut store = SqliteMetadataStore::open_path(self.db_path()).expect("open store");
        store.migrate().expect("migrate");
        store
    }

    fn audio_store(&self) -> FileAudioStore {
        FileAudioStore::new(self.root.join("audio"), self.root.join("decoded"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
