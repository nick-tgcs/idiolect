//! Integration test for delete-after-ship (S1): once a learning is shipped to
//! the PC, the phone flips its candidate to `synced`, drops ONLY the source
//! audio (never the row or transcript), so storage is reclaimed, the synced
//! learning leaves the manifest/outbox feed, and un-synced learnings are
//! untouched.
//!
//! Mirrors `training_retention.rs`: a real SQLite store + a real `FileAudioStore`
//! writing real files.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore, SqliteStorageErrorKind};
use idiolect_common::digest::audio_sha256_hex;
use idiolect_common::ids::ImeSessionId;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioObjectRef, AudioStorePort, MetadataStorePort};
use idiolect_test_support::fixtures::restart_traffic_fixture_16khz_mono;

#[test]
fn mark_synced_drops_only_that_audio_and_keeps_the_row_and_transcript() {
    let fixture = Fixture::new("reclaim");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    let (_a, a_audio) = seed_session(&mut store, &audio_store, "restart traffic");
    let (_b, b_audio) = seed_session(&mut store, &audio_store, "deploy traefik");

    let candidate_a = candidate_id_for(&store, "restart traffic");
    store
        .mark_synced_and_drop_audio(candidate_a, &audio_store)
        .expect("mark synced should succeed");

    // Storage reclaimed for the synced learning only.
    assert!(
        !audio_store
            .source_audio_exists_for_test(&a_audio)
            .expect("query"),
        "synced learning's audio is dropped"
    );
    assert!(
        audio_store
            .source_audio_exists_for_test(&b_audio)
            .expect("query"),
        "un-synced learning's audio is untouched"
    );

    // Row + transcript survive — we still know what was said.
    assert_eq!(
        store.training_candidate_count_for_test().expect("count"),
        2,
        "both candidate rows remain"
    );
    let history: Vec<String> = store
        .recent_history(50)
        .expect("history")
        .into_iter()
        .map(|entry| entry.text)
        .collect();
    assert!(history.contains(&"restart traffic".to_owned()));
    assert!(history.contains(&"deploy traefik".to_owned()));

    // The synced learning leaves the manifest feed (its audio is gone); the
    // un-synced one still trains.
    let manifest: Vec<String> = store
        .training_candidates_for_manifest_v2("default")
        .expect("manifest")
        .into_iter()
        .map(|candidate| candidate.corrected_transcript)
        .collect();
    assert_eq!(
        manifest,
        vec!["deploy traefik".to_owned()],
        "synced candidate excluded from manifest; captured one remains"
    );
}

#[test]
fn outbox_returns_only_unshipped_captured_candidates() {
    let fixture = Fixture::new("outbox");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    seed_session(&mut store, &audio_store, "restart traffic");
    seed_session(&mut store, &audio_store, "deploy traefik");

    let pending_before = pending_texts(&store);
    assert_eq!(
        pending_before.len(),
        2,
        "both captured candidates are pending"
    );

    let candidate_a = candidate_id_for(&store, "restart traffic");
    store
        .mark_synced_and_drop_audio(candidate_a, &audio_store)
        .expect("mark synced");

    assert_eq!(
        pending_texts(&store),
        vec!["deploy traefik".to_owned()],
        "synced candidate no longer appears in the outbox"
    );
}

#[test]
fn mark_synced_on_unknown_candidate_errors() {
    let fixture = Fixture::new("missing");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    let error = store
        .mark_synced_and_drop_audio(999_999, &audio_store)
        .expect_err("unknown candidate must error");
    assert_eq!(error.kind(), SqliteStorageErrorKind::Backend);
}

fn pending_texts(store: &SqliteMetadataStore) -> Vec<String> {
    store
        .training_candidates_pending_sync("default")
        .expect("outbox query")
        .into_iter()
        .map(|candidate| candidate.corrected_transcript)
        .collect()
}

fn candidate_id_for(store: &SqliteMetadataStore, corrected: &str) -> i64 {
    store
        .training_candidates_for_manifest_v2("default")
        .expect("candidates")
        .into_iter()
        .find(|candidate| candidate.corrected_transcript == corrected)
        .expect("candidate exists")
        .training_candidate_id
}

/// Commit a session with real audio on disk *and* its content digest populated
/// (mirroring real capture), then return its id and the audio ref.
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
    store
        .set_audio_digest(&utterance_id, &audio_sha256_hex(&encoded.payload))
        .expect("set audio digest");
    (session_id, audio_ref)
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
        let root = env::temp_dir().join(format!(
            "idiolect-sync-reclaim-{tag}-{}-{}",
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
