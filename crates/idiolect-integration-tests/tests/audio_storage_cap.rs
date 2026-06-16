//! Integration test for the captured-audio storage cap: `evict_captured_audio_over_cap`
//! reclaims the *oldest* captured audio until the stored source audio fits under the cap,
//! while keeping the dictation history (text) intact and leaving an evicted candidate out
//! of the sync outbox so a later pairing never ships a learning with missing audio.
//!
//! Mirrors `training_retention.rs`: a real SQLite store + a real `FileAudioStore` writing
//! real files; eviction order is oldest-first by candidate id (= capture order).

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
fn cap_evicts_oldest_captured_audio_and_keeps_history() {
    let fixture = Fixture::new("evict-oldest");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    // Three captures, oldest → newest, each with real audio on disk.
    let (_a, alpha) = seed_session(&mut store, &audio_store, "alpha take");
    let (_b, bravo) = seed_session(&mut store, &audio_store, "bravo take");
    let (_c, charlie) = seed_session(&mut store, &audio_store, "charlie take");

    // Cap that admits only the newest file (all three encode to the same size).
    let one = audio_store
        .source_audio_size_by_key(&charlie.object_key)
        .expect("size");
    assert!(one > 0, "fixture audio must be non-empty");

    let evicted = store
        .evict_captured_audio_over_cap("default", one, &audio_store)
        .expect("eviction runs");

    assert_eq!(evicted, 2, "the two oldest captures are evicted");
    assert!(!exists(&audio_store, &alpha), "oldest audio is reclaimed");
    assert!(
        !exists(&audio_store, &bravo),
        "second-oldest audio is reclaimed"
    );
    assert!(
        exists(&audio_store, &charlie),
        "newest audio survives under the cap"
    );

    // Evicted candidates leave the sync outbox (so a later pair never ships missing audio).
    let pending: Vec<String> = store
        .training_candidates_pending_sync("default")
        .expect("pending")
        .into_iter()
        .map(|c| c.corrected_transcript)
        .collect();
    assert_eq!(
        pending,
        vec!["charlie take".to_owned()],
        "only the newest stays pending"
    );

    // History (the user's dictation record) is untouched — only audio was reclaimed.
    let mut history: Vec<String> = store
        .recent_history(50)
        .expect("history")
        .into_iter()
        .map(|e| e.text)
        .collect();
    history.sort();
    assert_eq!(
        history,
        vec![
            "alpha take".to_owned(),
            "bravo take".to_owned(),
            "charlie take".to_owned()
        ],
        "all three transcripts remain in history",
    );
}

#[test]
fn cap_above_total_evicts_nothing() {
    let fixture = Fixture::new("under-cap");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    let (_a, alpha) = seed_session(&mut store, &audio_store, "alpha take");
    let (_b, bravo) = seed_session(&mut store, &audio_store, "bravo take");

    let total = audio_store
        .source_audio_size_by_key(&alpha.object_key)
        .expect("size")
        + audio_store
            .source_audio_size_by_key(&bravo.object_key)
            .expect("size");

    let evicted = store
        .evict_captured_audio_over_cap("default", total * 4, &audio_store)
        .expect("eviction runs");

    assert_eq!(
        evicted, 0,
        "nothing is evicted when comfortably under the cap"
    );
    assert!(exists(&audio_store, &alpha));
    assert!(exists(&audio_store, &bravo));
}

fn exists(audio_store: &FileAudioStore, audio_ref: &AudioObjectRef) -> bool {
    audio_store
        .source_audio_exists_for_test(audio_ref)
        .expect("exists query")
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

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
        let root = env::temp_dir().join(format!(
            "idiolect-audio-cap-{tag}-{}-{}",
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
