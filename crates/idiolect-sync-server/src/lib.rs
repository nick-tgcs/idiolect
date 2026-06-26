//! The PC side of learning-sync: take a decoded [`SyncBatchEnvelope`] and write
//! its learnings into the local store + audio store, so the existing trainer can
//! consume them unchanged.
//!
//! Transport-free on purpose — it takes an already-decoded envelope, so the same
//! ingest logic is exercised by an in-process test today and by the HTTP handler
//! later. Ingest is **content-addressed idempotent**: a learning whose
//! `audio_digest` is already stored is skipped (`already_have`), so a replayed
//! batch is a no-op.

use idiolect_adapter_sqlite::{
    FileAudioStore, FileAudioStoreError, SqliteMetadataStore, SqliteStorageError,
};
use idiolect_ports::audio::EncodedAudio;
use idiolect_ports::storage::{AudioStorePort, MetadataStorePort};
use idiolect_sync::SyncBatchEnvelope;

pub mod device_tokens;
pub mod host;
pub mod ingest_server;
pub mod model_server;
pub mod pairing;
pub mod pairing_qr;
pub mod tls;

use std::sync::Arc;

use ingest_server::{ingest_router, IngestServerState};
use model_server::{model_router, ModelServerConfig};
use pairing::{pair_router, PairingServerState};

/// Compose the full sync server: the model endpoints (M5), the pairing handshake (S3),
/// and — when `ingest` is configured — the learning-ingest endpoint (M6), all on one
/// router sharing the per-device token store. This is the single source of truth for
/// the app's shape: the `idiolect-sync-server` binary serves exactly this, and the
/// `tests/pairing_over_http.rs` integration test stands up exactly this on a real
/// socket, so the wire contract the phone/emulator hits is the one under test.
pub fn build_app(
    model: Arc<ModelServerConfig>,
    pairing: Arc<PairingServerState>,
    ingest: Option<Arc<IngestServerState>>,
) -> axum::Router {
    let mut app = model_router(model).merge(pair_router(pairing));
    if let Some(state) = ingest {
        app = app.merge(ingest_router(state));
    }
    app
}

/// The PC's local user. `SqliteMetadataStore::create_session` writes rows under
/// this id, so ingest must address audio and dedup under the same one.
const INGEST_USER_ID: &str = "default";

/// What an ingest did, keyed by `audio_digest`.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct IngestReport {
    /// Newly stored learnings.
    pub accepted: Vec<String>,
    /// Learnings whose audio digest was already present (idempotent skip).
    pub already_have: Vec<String>,
}

/// Failures while ingesting a batch.
#[derive(Debug, thiserror::Error)]
pub enum SyncServerError {
    #[error("storage error: {0}")]
    Storage(#[source] SqliteStorageError),
    #[error("audio store error: {0}")]
    Audio(#[source] FileAudioStoreError),
    #[error("batch references audio not included in the envelope: {0:?}")]
    MissingAudio(Vec<String>),
}

impl From<SqliteStorageError> for SyncServerError {
    fn from(error: SqliteStorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<FileAudioStoreError> for SyncServerError {
    fn from(error: FileAudioStoreError) -> Self {
        Self::Audio(error)
    }
}

/// Ingest a decoded batch into the local store + audio store. Each learning that
/// is new (by content digest) is recreated as a session → correction → committed
/// training candidate, with its audio written and digest set, so the existing
/// trainer picks it up unchanged. Already-present digests are skipped.
///
/// Rejects the whole batch up front if it references audio it didn't carry —
/// better to fail loudly than train on a hole.
pub fn ingest(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    envelope: &SyncBatchEnvelope,
) -> Result<IngestReport, SyncServerError> {
    let missing = envelope.missing_audio_digests();
    if !missing.is_empty() {
        return Err(SyncServerError::MissingAudio(missing));
    }

    let mut report = IngestReport::default();
    for learning in &envelope.batch.learnings {
        if store.has_utterance_with_digest(INGEST_USER_ID, &learning.audio_digest)? {
            report.already_have.push(learning.audio_digest.clone());
            continue;
        }

        let session_id = store.create_session(Some(&learning.raw_transcript))?;
        if learning.corrected_transcript != learning.raw_transcript {
            store.record_preedit_change(
                session_id,
                &learning.raw_transcript,
                &learning.corrected_transcript,
                0,
            )?;
        }
        let idempotency_key = format!(
            "ingest:{}:{}",
            envelope.batch.batch_id, learning.audio_digest
        );
        store.commit_session(session_id, &learning.corrected_transcript, &idempotency_key)?;

        let utterance_id = store.utterance_id_for_session(session_id)?;
        let payload = envelope
            .audio
            .get(&learning.audio_digest)
            .expect("missing_audio_digests checked every digest is present");
        let encoded = EncodedAudio {
            codec_name: "opus".to_owned(),
            sample_rate_hz: 16_000,
            channels: 1,
            payload: payload.clone(),
        };
        audio_store.write_source_audio(INGEST_USER_ID, &utterance_id, &encoded)?;
        store.set_audio_digest(&utterance_id, &learning.audio_digest)?;

        report.accepted.push(learning.audio_digest.clone());
    }
    Ok(report)
}
