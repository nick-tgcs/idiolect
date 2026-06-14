//! End-to-end learning-sync on one machine, no network (S2 logic): capture +
//! correct in data-root A (the "phone") → build a batch → run it through the
//! wire codec → ingest into data-root B (the "PC") → the corrections land as
//! trainable candidates on B with their audio intact → reclaim on A. Re-ingest
//! is idempotent (content-addressed by `audio_digest`).

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
use idiolect_sync_server::ingest;

#[test]
fn corrections_sync_from_phone_to_pc_and_reclaim_locally() {
    let phone = Fixture::new("phone");
    let mut phone_store = phone.open_store();
    let phone_audio = phone.audio_store();

    let pc = Fixture::new("pc");
    let mut pc_store = pc.open_store();
    let pc_audio = pc.audio_store();

    // Phone captures two dictations and corrects them.
    seed_candidate(
        &mut phone_store,
        &phone_audio,
        "restart trafic",
        "restart traffic",
        b"opus-A",
    );
    seed_candidate(
        &mut phone_store,
        &phone_audio,
        "deploy trafik",
        "deploy traefik",
        b"opus-B",
    );

    // Phone builds a batch; it travels the wire (encode -> bytes -> decode).
    let envelope =
        build_batch(&phone_store, &phone_audio, "default", "pixel", "batch-1").expect("build");
    let on_wire = encode_batch(&envelope).expect("encode");
    let received = decode_batch(&on_wire).expect("decode");

    // PC ingests.
    let report = ingest(&mut pc_store, &pc_audio, &received).expect("ingest");
    assert_eq!(report.accepted.len(), 2, "both learnings are new to the PC");
    assert!(report.already_have.is_empty());

    // The corrections are now trainable candidates on the PC, audio intact.
    let pc_candidates = pc_store
        .training_candidates_for_manifest_v2("default")
        .expect("pc manifest");
    assert_eq!(pc_candidates.len(), 2);
    let mut pc_texts: Vec<String> = pc_candidates
        .iter()
        .map(|candidate| candidate.corrected_transcript.clone())
        .collect();
    pc_texts.sort();
    assert_eq!(pc_texts, vec!["deploy traefik", "restart traffic"]);
    for candidate in &pc_candidates {
        assert!(!candidate.audio_digest.is_empty(), "digest carried over");
        let payload = pc_audio
            .read_source_payload_by_key(&candidate.audio_object_key)
            .expect("pc stored the audio");
        assert_eq!(
            audio_sha256_hex(&payload),
            candidate.audio_digest,
            "stored audio matches its content digest end-to-end"
        );
    }

    // Re-ingesting the same batch is a no-op (content-addressed idempotency).
    let replay = ingest(&mut pc_store, &pc_audio, &received).expect("re-ingest");
    assert!(replay.accepted.is_empty());
    assert_eq!(replay.already_have.len(), 2, "already had both digests");
    assert_eq!(
        pc_store
            .training_candidates_for_manifest_v2("default")
            .expect("pc manifest")
            .len(),
        2,
        "replay created no duplicate rows"
    );

    // Phone reclaims local storage now the PC has the data.
    confirm_shipped(&mut phone_store, &phone_audio, &envelope.batch.learnings).expect("confirm");
    assert!(
        phone_store
            .training_candidates_pending_sync("default")
            .expect("outbox")
            .is_empty(),
        "phone outbox drained after ship"
    );
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
            "idiolect-sync-rt-{tag}-{}-{}",
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
