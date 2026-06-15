//! The personal **sync ingest server**: accepts a phone's encoded learning batch
//! over HTTP (`POST /v1/sync`) and writes it into the local store + audio store via
//! [`crate::ingest`], so the existing trainer consumes it unchanged. This is the PC
//! half of the M6 learning round-trip, sitting beside the M5 `GET /model` hop.
//!
//! Like the model server, all logic lives in the router and is gate-tested
//! deterministically via `tower::ServiceExt::oneshot`; [`serve_ingest`] is the thin
//! socket glue. Desktop-only: `axum` is never linked into the Android `.so`.
//!
//! Bearer-token authenticated — a single shared secret for v1; S3 replaces the
//! constant-compare with a per-device token → device/user lookup (the same change
//! the model server's `authorized` will take, so the two should share it then).
//!
//! The request body is the length-prefixed sync container
//! (`application/vnd.idiolect.sync.v1`, see [`idiolect_sync::codec`]); the response is
//! a JSON ack listing the `accepted` and `already_have` audio digests. Both are now
//! durably stored on the PC, so the phone reclaims local storage for either
//! (delete-after-ACK). Ingest is content-addressed idempotent, so a replayed batch
//! acks every digest under `already_have` and creates no duplicate rows.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_sync::decode_batch;
use serde::Serialize;

use crate::{ingest, SyncServerError};

/// Cap a single uploaded batch. A personal outbox is small (opus is a few KB/s) but
/// can accumulate while the phone is offline; 128 MiB is a generous bound that still
/// rejects a runaway/hostile body. The whole batch is held in memory to decode.
const MAX_BATCH_BYTES: usize = 128 * 1024 * 1024;

/// Shared state for the ingest server: the local store (mutated under a lock — a
/// personal server serves ~one phone, so serialised ingest is fine) plus the audio
/// store and the bearer token.
pub struct IngestServerState {
    store: Mutex<SqliteMetadataStore>,
    audio_store: FileAudioStore,
    bearer_token: String,
}

impl IngestServerState {
    /// Wrap an already-opened (migrated) store + audio store behind the ingest API.
    #[must_use]
    pub fn new(
        store: SqliteMetadataStore,
        audio_store: FileAudioStore,
        bearer_token: String,
    ) -> Self {
        Self {
            store: Mutex::new(store),
            audio_store,
            bearer_token,
        }
    }
}

/// The ack the phone receives: which learnings are now durably stored on the PC, so
/// it can reclaim them. Mirrors [`crate::IngestReport`] on the wire.
#[derive(Debug, Serialize)]
pub struct IngestAck {
    pub accepted: Vec<String>,
    pub already_have: Vec<String>,
}

/// Build the ingest router: `POST /v1/sync`, bearer-guarded, body = sync container.
pub fn ingest_router(state: Arc<IngestServerState>) -> Router {
    Router::new()
        .route("/v1/sync", post(ingest_batch))
        .layer(DefaultBodyLimit::max(MAX_BATCH_BYTES))
        .with_state(state)
}

/// Bind the ingest router to `listener` and serve until the process ends.
pub async fn serve_ingest(
    listener: tokio::net::TcpListener,
    state: Arc<IngestServerState>,
) -> std::io::Result<()> {
    axum::serve(listener, ingest_router(state)).await
}

fn authorized(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected)
}

async fn ingest_batch(
    State(state): State<Arc<IngestServerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&headers, &state.bearer_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let envelope = match decode_batch(body.as_ref()) {
        Ok(envelope) => envelope,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let report = {
        let mut store = state.store.lock().expect("ingest store mutex poisoned");
        ingest(&mut store, &state.audio_store, &envelope)
    };
    match report {
        Ok(report) => Json(IngestAck {
            accepted: report.accepted,
            already_have: report.already_have,
        })
        .into_response(),
        // Well-formed container, but it references audio it didn't carry — reject the
        // whole batch rather than train on a hole.
        Err(SyncServerError::MissingAudio(_)) => StatusCode::UNPROCESSABLE_ENTITY.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use idiolect_sync::{encode_batch, SyncBatch, SyncBatchEnvelope, SyncLearning};
    use tower::ServiceExt;

    const TOKEN: &str = "ingest-token";

    fn state() -> (tempfile::TempDir, Arc<IngestServerState>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store =
            SqliteMetadataStore::open_path(dir.path().join("idiolect.sqlite")).expect("open store");
        store.migrate().expect("migrate");
        let audio = FileAudioStore::new(dir.path().join("audio"), dir.path().join("decoded"));
        let state = Arc::new(IngestServerState::new(store, audio, TOKEN.to_owned()));
        (dir, state)
    }

    fn learning(digest: &str, raw: &str, corrected: &str) -> SyncLearning {
        SyncLearning {
            training_candidate_id: 1,
            user_id: "default".to_owned(),
            utterance_id: format!("u-{digest}"),
            text_session_id: format!("s-{digest}"),
            audio_object_key: format!("k-{digest}"),
            audio_digest: digest.to_owned(),
            raw_transcript: raw.to_owned(),
            corrected_transcript: corrected.to_owned(),
            source_label: "ime".to_owned(),
            trust_score_bps: 10_000,
        }
    }

    fn batch_bytes(learnings: Vec<SyncLearning>, audio: BTreeMap<String, Vec<u8>>) -> Vec<u8> {
        let envelope = SyncBatchEnvelope::new(
            SyncBatch {
                device_id: "pixel".to_owned(),
                batch_id: "batch-1".to_owned(),
                learnings,
            },
            audio,
        );
        encode_batch(&envelope).expect("encode")
    }

    fn post(body: Vec<u8>, auth: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("POST").uri("/v1/sync");
        if let Some(token) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::from(body)).expect("request")
    }

    async fn ack(response: Response) -> serde_json::Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("ack json")
    }

    #[tokio::test]
    async fn an_unauthenticated_post_is_rejected() {
        let (_dir, state) = state();
        let body = batch_bytes(vec![], BTreeMap::new());
        for auth in [None, Some("wrong")] {
            let response = ingest_router(state.clone())
                .oneshot(post(body.clone(), auth))
                .await
                .expect("router");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn a_batch_is_ingested_and_its_digests_acked() {
        let (_dir, state) = state();
        let mut audio = BTreeMap::new();
        audio.insert("digest-a".to_owned(), b"opus-A".to_vec());
        let body = batch_bytes(
            vec![learning("digest-a", "restart trafic", "restart traffic")],
            audio,
        );

        let response = ingest_router(state.clone())
            .oneshot(post(body, Some(TOKEN)))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::OK);
        let ack = ack(response).await;
        assert_eq!(ack["accepted"], serde_json::json!(["digest-a"]));
        assert_eq!(ack["already_have"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn a_replayed_batch_is_idempotent() {
        let (_dir, state) = state();
        let mut audio = BTreeMap::new();
        audio.insert("digest-a".to_owned(), b"opus-A".to_vec());
        let body = batch_bytes(vec![learning("digest-a", "raw", "corrected")], audio);

        let first = ingest_router(state.clone())
            .oneshot(post(body.clone(), Some(TOKEN)))
            .await
            .expect("router");
        assert_eq!(first.status(), StatusCode::OK);

        let second = ingest_router(state.clone())
            .oneshot(post(body, Some(TOKEN)))
            .await
            .expect("router");
        assert_eq!(second.status(), StatusCode::OK);
        let ack = ack(second).await;
        assert_eq!(ack["accepted"], serde_json::json!([]));
        assert_eq!(ack["already_have"], serde_json::json!(["digest-a"]));
    }

    #[tokio::test]
    async fn a_malformed_body_is_a_bad_request() {
        let (_dir, state) = state();
        let response = ingest_router(state)
            .oneshot(post(b"not a sync container".to_vec(), Some(TOKEN)))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_batch_missing_its_audio_is_unprocessable() {
        let (_dir, state) = state();
        // The learning references "digest-x" but the envelope carries no audio for it.
        let body = batch_bytes(
            vec![learning("digest-x", "raw", "corrected")],
            BTreeMap::new(),
        );
        let response = ingest_router(state)
            .oneshot(post(body, Some(TOKEN)))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
