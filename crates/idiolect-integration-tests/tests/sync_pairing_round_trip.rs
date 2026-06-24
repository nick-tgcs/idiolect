//! S3 pairing round-trip: a device with no token mints one through the typed pairing
//! code, and that single per-device token then authenticates BOTH the ingest
//! (`POST /v1/sync`) and model (`GET /v1/model`) endpoints sharing one token store —
//! the "safe to expose on the tailnet" exit criterion as a green test. Also pins the
//! `IDIOLECT_SYNC_TOKEN` backcompat through the `Arc<Mutex<DeviceTokenStore>>` migration.
//!
//! Routers are driven via `tower::oneshot` (in-process), the same deterministic seam the
//! M5/M6 HTTP tests use; the real socket is exercised by the M6 emulator e2e.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_sync::{encode_batch, SyncBatch, SyncBatchEnvelope, SyncLearning};
use idiolect_sync_server::device_tokens::DeviceTokenStore;
use idiolect_sync_server::ingest_server::{ingest_router, IngestServerState};
use idiolect_sync_server::model_server::{model_router, ModelServerConfig};
use idiolect_sync_server::pairing::{pair_router, system_now, PairingServerState};
use tower::ServiceExt;

#[tokio::test]
async fn an_unpaired_server_rejects_sync_until_a_code_is_redeemed() {
    let pc = Fixture::new();
    let ingest = pc.ingest_state();

    // Zero devices paired: the ingest endpoint is closed even to a well-formed batch.
    let before = ingest_router(ingest.clone())
        .oneshot(sync_request(None, batch_bytes()))
        .await
        .expect("router");
    assert_eq!(before.status(), StatusCode::UNAUTHORIZED);

    // The operator mints a code; the phone redeems it for a token.
    let token = pc.pair("pixel").await;

    let after = ingest_router(ingest)
        .oneshot(sync_request(Some(&token), batch_bytes()))
        .await
        .expect("router");
    assert_eq!(after.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_paired_token_authenticates_both_sync_and_model() {
    let pc = Fixture::new();
    let ingest = pc.ingest_state();
    let model = pc.model_config();

    let token = pc.pair("pixel-7a").await;

    // The one issued token ingests a learning batch...
    let synced = ingest_router(ingest)
        .oneshot(sync_request(Some(&token), batch_bytes()))
        .await
        .expect("router");
    assert_eq!(synced.status(), StatusCode::OK);
    let ack = body_json(synced).await;
    assert_eq!(
        ack["accepted"].as_array().expect("accepted").len(),
        1,
        "the paired device's learning landed on the PC"
    );

    // ...and pulls the model manifest, both against the same shared token store.
    let manifest = model_router(model)
        .oneshot(manifest_request(&token))
        .await
        .expect("router");
    assert_eq!(manifest.status(), StatusCode::OK);
    assert_eq!(body_json(manifest).await["id"], "base.en");
}

#[tokio::test]
async fn the_legacy_env_token_still_authenticates_after_the_mutex_change() {
    let pc = Fixture::new();
    // Mirror the binary's IDIOLECT_SYNC_TOKEN wiring: a shared secret bound to a
    // default device in the same store the routers now reach through a Mutex.
    pc.tokens
        .lock()
        .expect("tokens")
        .bind("legacy-secret", "default-device", "default")
        .expect("bind");

    let synced = ingest_router(pc.ingest_state())
        .oneshot(sync_request(Some("legacy-secret"), batch_bytes()))
        .await
        .expect("router");
    assert_eq!(synced.status(), StatusCode::OK);

    let manifest = model_router(pc.model_config())
        .oneshot(manifest_request("legacy-secret"))
        .await
        .expect("router");
    assert_eq!(manifest.status(), StatusCode::OK);
}

/// A personal PC server: one shared token store behind every router, over a temp root.
struct Fixture {
    root: PathBuf,
    tokens: Arc<Mutex<DeviceTokenStore>>,
}

impl Fixture {
    fn new() -> Self {
        // A unique root per fixture. process::id() + nanos alone can collide across
        // parallel tests that read the clock at the same instant (seen under coverage
        // instrumentation: two tests shared one SQLite db and raced migrate/cleanup,
        // surfacing "schema_migrations already exists" / "readonly database"), so a
        // process-wide counter makes collisions impossible by construction.
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
        let root = std::env::temp_dir().join(format!(
            "idiolect-pairing-{}-{}-{}",
            std::process::id(),
            now.as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&root).expect("fixture root");
        let tokens = DeviceTokenStore::open(root.join("device-tokens.json")).expect("tokens");
        Self {
            root,
            tokens: Arc::new(Mutex::new(tokens)),
        }
    }

    /// Mint a code and redeem it over `POST /v1/pair`, returning the issued bearer token.
    async fn pair(&self, device_id: &str) -> String {
        let pairing = Arc::new(PairingServerState::new(Arc::clone(&self.tokens)));
        let code = pairing.generate_code(system_now());
        let response = pair_router(pairing)
            .oneshot(pair_request(&code, device_id))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::CREATED, "pairing failed");
        body_json(response).await["token"]
            .as_str()
            .expect("token")
            .to_owned()
    }

    fn ingest_state(&self) -> Arc<IngestServerState> {
        let mut store =
            SqliteMetadataStore::open_path(self.root.join("idiolect.sqlite")).expect("open store");
        store.migrate().expect("migrate");
        let audio = FileAudioStore::new(self.root.join("audio"), self.root.join("decoded"));
        Arc::new(IngestServerState::new(
            store,
            audio,
            Arc::clone(&self.tokens),
        ))
    }

    fn model_config(&self) -> Arc<ModelServerConfig> {
        let model_path = self.root.join("model.bin");
        std::fs::write(&model_path, b"trained-model-bytes").expect("write model");
        Arc::new(ModelServerConfig {
            model_path,
            model_id: "base.en".to_owned(),
            tokens: Arc::clone(&self.tokens),
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A minimal, well-formed one-learning batch with its audio carried inline.
fn batch_bytes() -> Vec<u8> {
    let mut audio = BTreeMap::new();
    audio.insert("digest-a".to_owned(), b"opus-A".to_vec());
    let envelope = SyncBatchEnvelope::new(
        SyncBatch {
            device_id: "pixel".to_owned(),
            batch_id: "batch-1".to_owned(),
            learnings: vec![SyncLearning {
                training_candidate_id: 1,
                user_id: "default".to_owned(),
                utterance_id: "u-a".to_owned(),
                text_session_id: "s-a".to_owned(),
                audio_object_key: "k-a".to_owned(),
                audio_digest: "digest-a".to_owned(),
                raw_transcript: "restart trafic".to_owned(),
                corrected_transcript: "restart traffic".to_owned(),
                source_label: "ime".to_owned(),
                trust_score_bps: 10_000,
            }],
        },
        audio,
    );
    encode_batch(&envelope).expect("encode")
}

fn sync_request(token: Option<&str>, body: Vec<u8>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri("/v1/sync");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    builder.body(Body::from(body)).expect("request")
}

fn manifest_request(token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/v1/model/manifest")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request")
}

fn pair_request(code: &str, device_id: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/pair")
        .body(Body::from(
            serde_json::json!({ "code": code, "device_id": device_id }).to_string(),
        ))
        .expect("request")
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json")
}
