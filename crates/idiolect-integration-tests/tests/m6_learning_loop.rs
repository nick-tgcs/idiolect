//! M6 capstone: the whole personalisation loop, end to end, in one test.
//!
//!   phone captures corrections
//!     → ships them over the `POST /v1/sync` HTTP hop
//!       → the PC ingests them as trainable candidates
//!         → `trainerctl` LoRA-trains a merged model and atomically installs it into
//!           the live served slot
//!           → the phone pulls the NEW model over `GET /v1/model`, and the digest the
//!             server advertises has changed.
//!
//! The HTTP hops run through the real axum routers via `tower::oneshot` — the same
//! deterministic seam the model and ingest servers are unit-tested through (`serve` /
//! `serve_ingest` are one-line `axum::serve` wrappers, and the router carries all the
//! logic). The trainer runs in-process via its library entry point against the bundled
//! tiny fixture model, so the entire loop runs in CI with no GPU and no network. The
//! literal on-emulator run (the Kotlin transports over a real socket) is the manual
//! Android e2e; those client hops are host-tested in the app module.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_ports::audio::{AudioSegment, EncodedAudio};
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioStorePort, MetadataStorePort};
use idiolect_sync::encode_batch;
use idiolect_sync_client::build_batch;
use idiolect_sync_server::device_tokens::DeviceTokenStore;
use idiolect_sync_server::ingest_server::{ingest_router, IngestServerState};
use idiolect_sync_server::model_server::{model_router, ModelServerConfig};
use idiolect_test_support::fixtures::{
    restart_traffic_fixture_16khz_mono, sine_fixture_16khz_mono,
};
use idiolect_trainer_burn::ggml::GgmlModel;
use idiolect_trainerctl::train_command::{run_train, TrainFlags};
use tower::ServiceExt;

const TOKEN: &str = "m6-loop-token";

fn base_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/whisper/ggml-tiny.en.bin")
}

#[tokio::test]
async fn corrections_train_a_new_model_the_phone_then_pulls() {
    // ── Phone: capture two corrections over two *distinct* recordings (distinct
    // audio digests, so content-addressed ingest accepts both and the trainer
    // gets two samples). The audio is real opus the PC can decode and train on.
    let phone = Fixture::new("phone");
    let mut phone_store = phone.open_store();
    let phone_audio = phone.audio_store();
    seed_candidate(
        &mut phone_store,
        &phone_audio,
        &restart_traffic_fixture_16khz_mono(),
        "restart trafic",
        "restart traffic",
    );
    seed_candidate(
        &mut phone_store,
        &phone_audio,
        &sine_fixture_16khz_mono(),
        "deploy trafik",
        "deploy traefik",
    );

    // ── PC: ingest server over the same data root the trainer will read, guarded by
    // a per-device token store that also guards the model endpoint (one token, both
    // hops — S3). The model server serves the live slot, seeded with today's model.
    let pc = Fixture::new("pc");
    let tokens = {
        let mut store = DeviceTokenStore::open(pc.root.join("device-tokens.json")).expect("tokens");
        store.bind(TOKEN, "pixel", "default").expect("bind token");
        Arc::new(Mutex::new(store))
    };
    let ingest = Arc::new(IngestServerState::new(
        pc.open_store(),
        pc.audio_store(),
        Arc::clone(&tokens),
    ));

    let served = pc.root.join("served-model.bin");
    fs::copy(base_model_path(), &served).expect("seed the live model slot");
    let model = Arc::new(ModelServerConfig {
        model_path: served.clone(),
        model_id: "personal.en".to_owned(),
        tokens: Arc::clone(&tokens),
    });

    // ── Hop 1: the phone ships its batch over HTTP; the PC ingests both learnings.
    let envelope = build_batch(&phone_store, &phone_audio, "default", "pixel", "batch-1")
        .expect("build batch");
    let on_wire = encode_batch(&envelope).expect("encode batch");
    let (status, ack) = post_sync(ingest.clone(), on_wire).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ack["accepted"].as_array().expect("accepted").len(),
        2,
        "both corrections are new to the PC: {ack}"
    );

    // The model the phone is serving today, before any training.
    let digest_before = model_manifest(model.clone()).await["sha256"]
        .as_str()
        .expect("digest before")
        .to_owned();
    // A pull without the bearer token is refused (the loop respects S3 end to end).
    assert_eq!(
        model_router(model.clone())
            .oneshot(get("/v1/model", None))
            .await
            .expect("router")
            .status(),
        StatusCode::UNAUTHORIZED,
    );

    // ── Hop 2: the PC trains on the ingested corrections and atomically installs the
    // merged model into the live slot the running model server reads per request.
    let report = run_train(&TrainFlags {
        db: pc.db_path().to_str().expect("utf8").to_owned(),
        audio_root: pc.audio_root().to_str().expect("utf8").to_owned(),
        user: "default".to_owned(),
        base_model: base_model_path().to_str().expect("utf8").to_owned(),
        output: pc
            .root
            .join("personal.bin")
            .to_str()
            .expect("utf8")
            .to_owned(),
        epochs: 1,
        learning_rate: 1e-3,
        rank: 8,
        max_samples: Some(2),
        gpu: false,
        serve: Some(served.to_str().expect("utf8").to_owned()),
    })
    .expect("training + serve-swap succeeds");
    assert_eq!(
        report.usable_samples, 2,
        "both ingested corrections are trained"
    );
    assert!(
        report.applied,
        "the artifact was installed into the live slot"
    );

    // ── Hop 3: the phone pulls again. The SAME running server (it reads the slot per
    // request) now advertises a different digest and serves the new bytes.
    let manifest_after = model_manifest(model.clone()).await;
    let digest_after = manifest_after["sha256"].as_str().expect("digest after");
    assert_ne!(
        digest_after, digest_before,
        "the served model changed — the phone pulls the improved model"
    );
    assert_eq!(
        digest_after,
        idiolect_common::digest::file_sha256_hex(&served)
            .expect("digest the slot")
            .as_str(),
        "the manifest digest is the truth about the live slot"
    );

    let downloaded = model_download(model).await;
    assert_eq!(
        downloaded,
        fs::read(&served).expect("read slot"),
        "the bytes the phone pulls are exactly the live slot's"
    );
    assert_eq!(
        manifest_after["size"].as_u64().expect("size"),
        downloaded.len() as u64,
        "the advertised size matches the bytes served"
    );
    // What the phone just pulled is a structurally valid whisper model with base dims.
    let pulled = GgmlModel::read_file(&served).expect("pulled model parses as ggml");
    let base = GgmlModel::read_file(&base_model_path()).expect("base parses");
    assert_eq!(pulled.hparams, base.hparams);
    assert_eq!(pulled.tensors.len(), base.tensors.len());
}

/// POST an encoded batch through the ingest router and return `(status, ack json)`.
async fn post_sync(
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
    let bytes = collect(response).await;
    let json = serde_json::from_slice(&bytes).expect("ack json");
    (status, json)
}

/// `GET /v1/model/manifest` with the bearer token; returns the parsed JSON.
async fn model_manifest(config: Arc<ModelServerConfig>) -> serde_json::Value {
    let response = model_router(config)
        .oneshot(get("/v1/model/manifest", Some(TOKEN)))
        .await
        .expect("router");
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&collect(response).await).expect("manifest json")
}

/// `GET /v1/model` with the bearer token; returns the model bytes.
async fn model_download(config: Arc<ModelServerConfig>) -> Vec<u8> {
    let response = model_router(config)
        .oneshot(get("/v1/model", Some(TOKEN)))
        .await
        .expect("router");
    assert_eq!(response.status(), StatusCode::OK);
    collect(response).await
}

fn get(uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = auth {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    builder.body(Body::empty()).expect("request")
}

async fn collect(response: axum::response::Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes()
        .to_vec()
}

/// Capture a correction on the phone: a committed session whose raw draft was edited
/// to the corrected text, with real opus audio on disk and its content digest set —
/// exactly the shape `build_batch` ships and the PC trainer consumes.
fn seed_candidate(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    segment: &AudioSegment,
    raw: &str,
    corrected: &str,
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

    let encoded = OpusCodec::new().encode(segment).expect("encode fixture");
    let digest = idiolect_common::digest::audio_sha256_hex(&encoded.payload);
    let stored = EncodedAudio {
        codec_name: "opus".to_owned(),
        sample_rate_hz: 16_000,
        channels: 1,
        payload: encoded.payload,
    };
    audio_store
        .write_source_audio("default", &utterance_id, &stored)
        .expect("write source audio");
    store
        .set_audio_digest(&utterance_id, &digest)
        .expect("set audio digest");
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
        let root = env::temp_dir().join(format!(
            "idiolect-m6-loop-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root");
        Self { root }
    }

    fn db_path(&self) -> PathBuf {
        self.root.join("idiolect.sqlite")
    }

    fn audio_root(&self) -> PathBuf {
        self.root.join("audio")
    }

    fn open_store(&self) -> SqliteMetadataStore {
        let mut store = SqliteMetadataStore::open_path(self.db_path()).expect("open store");
        store.migrate().expect("migrate");
        store
    }

    fn audio_store(&self) -> FileAudioStore {
        FileAudioStore::new(self.audio_root(), self.root.join("decoded"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
