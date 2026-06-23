//! Integration test for the phone-side sync client (S2 client half): gather the
//! local outbox into a `SyncBatchEnvelope` (whose audio is content-addressed and
//! round-trips through the wire codec), then — once the PC confirms the bytes —
//! reclaim the local audio and clear the outbox.
//!
//! Audio is written with distinct synthetic payloads so the two learnings get
//! distinct `audio_digest`s (the shared fixture would collide them).

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_common::digest::audio_sha256_hex;
use idiolect_ports::audio::EncodedAudio;
use idiolect_ports::storage::{AudioStorePort, MetadataStorePort};
use idiolect_sync::{decode_batch, encode_batch};
use idiolect_sync_client::{build_batch, confirm_shipped};

#[test]
fn build_batch_collects_pending_learnings_with_content_addressed_audio() {
    let fixture = Fixture::new("build");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    seed_candidate(
        &mut store,
        &audio_store,
        "restart trafic",
        "restart traffic",
        b"opus-A",
    );
    seed_candidate(
        &mut store,
        &audio_store,
        "deploy trafik",
        "deploy traefik",
        b"opus-B",
    );

    let envelope = build_batch(&store, &audio_store, "default", "pixel", "batch-1").expect("build");

    assert_eq!(envelope.batch.device_id, "pixel");
    assert_eq!(envelope.batch.batch_id, "batch-1");
    assert_eq!(envelope.batch.learnings.len(), 2);
    // Audio rides content-addressed, one blob per distinct digest.
    assert_eq!(envelope.audio.len(), 2);
    assert!(envelope.missing_audio_digests().is_empty());
    assert_eq!(
        envelope
            .audio
            .get(&audio_sha256_hex(b"opus-A"))
            .map(Vec::as_slice),
        Some(b"opus-A".as_slice()),
    );
    // Corrected text (not raw) is what ships as the learning's target.
    let corrected: Vec<&str> = envelope
        .batch
        .learnings
        .iter()
        .map(|learning| learning.corrected_transcript.as_str())
        .collect();
    assert!(corrected.contains(&"restart traffic"));
    assert!(corrected.contains(&"deploy traefik"));

    // The whole envelope survives the wire codec untouched.
    let bytes = encode_batch(&envelope).expect("encode");
    assert_eq!(decode_batch(&bytes).expect("decode"), envelope);
}

#[test]
fn confirm_shipped_reclaims_audio_and_clears_the_outbox() {
    let fixture = Fixture::new("confirm");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    seed_candidate(
        &mut store,
        &audio_store,
        "restart trafic",
        "restart traffic",
        b"opus-A",
    );
    seed_candidate(
        &mut store,
        &audio_store,
        "deploy trafik",
        "deploy traefik",
        b"opus-B",
    );

    let envelope = build_batch(&store, &audio_store, "default", "pixel", "batch-1").expect("build");
    confirm_shipped(&mut store, &audio_store, &envelope.batch.learnings).expect("confirm");

    // Outbox drained: nothing left to ship.
    assert!(
        store
            .training_candidates_pending_sync("default")
            .expect("outbox")
            .is_empty(),
        "every shipped candidate left the outbox"
    );
    // Local audio reclaimed: rebuilding now finds no payloads to attach.
    let rebuilt =
        build_batch(&store, &audio_store, "default", "pixel", "batch-2").expect("rebuild");
    assert!(
        rebuilt.batch.learnings.is_empty(),
        "no pending learnings remain"
    );
    assert!(rebuilt.audio.is_empty(), "no local audio remains");
}

fn seed_candidate(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    raw: &str,
    corrected: &str,
    payload: &[u8],
) {
    let session_id = store.create_session(Some(raw)).expect("create session");
    if raw != corrected {
        store
            .record_preedit_change(session_id, raw, corrected, 0)
            .expect("record correction");
    }
    store
        .commit_session(session_id, corrected, &format!("commit-{corrected}"))
        .expect("commit session");
    let utterance_id = store
        .session_utterance_link_for_test(session_id)
        .expect("session link query")
        .expect("session has utterance")
        .utterance_id;

    let encoded = EncodedAudio {
        codec_name: "opus".to_owned(),
        sample_rate_hz: 16_000,
        channels: 1,
        payload: payload.to_vec(),
    };
    audio_store
        .write_source_audio("default", &utterance_id, &encoded)
        .expect("write source audio");
    store
        .set_audio_digest(&utterance_id, &audio_sha256_hex(payload))
        .expect("set audio digest");
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
        let root = env::temp_dir().join(format!(
            "idiolect-sync-client-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root");
        Self { root }
    }

    fn open_store(&self) -> SqliteMetadataStore {
        let mut store =
            SqliteMetadataStore::open_path(self.root.join("idiolect.sqlite")).expect("open store");
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
