//! M6: the learning round-trip over the actual HTTP ingest hop. Mirrors
//! `sync_round_trip.rs` (the transport-free S2 logic) but ships the batch through
//! the `POST /v1/sync` axum router — proving the phone client and PC server agree
//! over the wire encoding *and* the HTTP request/response framing, and that the
//! JSON ack drives the phone's delete-after-ACK reclaim.
//!
//! The router is driven via `tower::oneshot` (in-process), the same deterministic
//! seam the model server is tested through; `serve_ingest` itself is a one-line
//! `axum::serve` wrapper and the real socket is exercised by the M6 emulator e2e.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_ports::audio::EncodedAudio;
use idiolect_ports::storage::{AudioStorePort, MetadataStorePort};
use idiolect_sync::encode_batch;
use idiolect_sync_client::{build_batch, confirm_shipped};
use idiolect_sync_server::ingest_server::{ingest_router, IngestServerState};
use tower::ServiceExt;

const TOKEN: &str = "round-trip-token";

#[tokio::test]
async fn corrections_sync_phone_to_pc_over_http_then_reclaim() {
    let phone = Fixture::new("phone");
    let mut phone_store = phone.open_store();
    let phone_audio = phone.audio_store();

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

    // The PC stands up its ingest server over the same data root the trainer reads.
    let pc = Fixture::new("pc");
    let pc_state = Arc::new(IngestServerState::new(
        pc.open_store(),
        pc.audio_store(),
        TOKEN.to_owned(),
    ));

    // Phone builds a batch and ships it over HTTP.
    let envelope =
        build_batch(&phone_store, &phone_audio, "default", "pixel", "batch-1").expect("build");
    let on_wire = encode_batch(&envelope).expect("encode");

    let (status, ack) = post_batch(pc_state.clone(), on_wire.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ack["accepted"].as_array().expect("accepted").len(),
        2,
        "both learnings are new to the PC"
    );
    assert!(ack["already_have"]
        .as_array()
        .expect("already_have")
        .is_empty());

    // Re-POSTing the same batch is idempotent over HTTP (content-addressed dedup).
    let (status, ack) = post_batch(pc_state.clone(), on_wire).await;
    assert_eq!(status, StatusCode::OK);
    assert!(ack["accepted"].as_array().expect("accepted").is_empty());
    assert_eq!(
        ack["already_have"].as_array().expect("already_have").len(),
        2,
        "replay acks both digests as already-stored"
    );

    // The corrections landed on the PC as trainable candidates, audio intact.
    let pc_inspect = pc.open_store();
    let pc_candidates = pc_inspect
        .training_candidates_for_manifest_v2("default")
        .expect("pc manifest");
    assert_eq!(pc_candidates.len(), 2, "replay created no duplicate rows");
    let mut texts: Vec<String> = pc_candidates
        .iter()
        .map(|candidate| candidate.corrected_transcript.clone())
        .collect();
    texts.sort();
    assert_eq!(texts, vec!["deploy traefik", "restart traffic"]);

    // Phone reclaims local storage now the PC has acked.
    confirm_shipped(&mut phone_store, &phone_audio, &envelope.batch.learnings).expect("confirm");
    assert!(
        phone_store
            .training_candidates_pending_sync("default")
            .expect("outbox")
            .is_empty(),
        "phone outbox drained after the ACK"
    );
}

/// POST an encoded batch through the ingest router and return `(status, ack json)`.
async fn post_batch(
    state: Arc<IngestServerState>,
    body: Vec<u8>,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/sync")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::from(body))
        .expect("request");
    let response = ingest_router(state).oneshot(request).await.expect("router");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("ack json")
    };
    (status, json)
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
        .set_audio_digest(
            &utterance_id,
            &idiolect_common::digest::audio_sha256_hex(payload),
        )
        .expect("set audio digest");
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
        let root = env::temp_dir().join(format!(
            "idiolect-sync-http-{tag}-{}-{}",
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
