//! The phone side of learning-sync: gather the local outbox into a
//! [`SyncBatchEnvelope`] ready to ship, and — once the PC has confirmed the
//! bytes are durably stored — reclaim the local audio.
//!
//! Deliberately transport-free: it produces/consumes the [`idiolect_sync`]
//! envelope but knows nothing about HTTP. The actual POST and the ACK handling
//! live in the runtime that drives this (so the same logic works over Tailscale
//! or the LAN mDNS fallback).

use std::collections::BTreeMap;

use idiolect_adapter_sqlite::repository::ManifestV2TrainingCandidate;
use idiolect_adapter_sqlite::{
    FileAudioStore, FileAudioStoreError, SqliteMetadataStore, SqliteStorageError,
};
use idiolect_sync::{SyncBatch, SyncBatchEnvelope, SyncLearning};

/// Failures while building a batch or reclaiming after a ship.
#[derive(Debug, thiserror::Error)]
pub enum SyncClientError {
    #[error("storage error: {0}")]
    Storage(#[source] SqliteStorageError),
    #[error("audio store error: {0}")]
    Audio(#[source] FileAudioStoreError),
}

impl From<SqliteStorageError> for SyncClientError {
    fn from(error: SqliteStorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<FileAudioStoreError> for SyncClientError {
    fn from(error: FileAudioStoreError) -> Self {
        Self::Audio(error)
    }
}

/// Gather everything in the local sync outbox (captured, not-yet-shipped
/// candidates) into a [`SyncBatchEnvelope`]: one [`SyncLearning`] per candidate,
/// plus its encoded audio attached content-addressed by `audio_digest` (so a
/// digest shared by two candidates ships its bytes once).
pub fn build_batch(
    store: &SqliteMetadataStore,
    audio_store: &FileAudioStore,
    user_id: &str,
    device_id: &str,
    batch_id: &str,
) -> Result<SyncBatchEnvelope, SyncClientError> {
    let candidates = store.training_candidates_pending_sync(user_id)?;

    let mut learnings = Vec::with_capacity(candidates.len());
    let mut audio: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for candidate in candidates {
        if !audio.contains_key(&candidate.audio_digest) {
            let payload = audio_store.read_source_payload_by_key(&candidate.audio_object_key)?;
            audio.insert(candidate.audio_digest.clone(), payload);
        }
        learnings.push(learning_from_candidate(candidate));
    }

    Ok(SyncBatchEnvelope::new(
        SyncBatch {
            device_id: device_id.to_owned(),
            batch_id: batch_id.to_owned(),
            learnings,
        },
        audio,
    ))
}

/// After the PC confirms a batch is durably stored, flip each shipped candidate
/// to `synced` and drop its local audio (delete-after-ship). Keyed by the
/// learning's `training_candidate_id`, which is the *local* id — confirmation
/// runs on the same device that built the batch.
pub fn confirm_shipped(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    learnings: &[SyncLearning],
) -> Result<(), SyncClientError> {
    for learning in learnings {
        store.mark_synced_and_drop_audio(learning.training_candidate_id, audio_store)?;
    }
    Ok(())
}

/// Project a storage candidate row onto the wire learning (drops `split` — the
/// PC owns that).
fn learning_from_candidate(candidate: ManifestV2TrainingCandidate) -> SyncLearning {
    SyncLearning {
        training_candidate_id: candidate.training_candidate_id,
        user_id: candidate.user_id,
        utterance_id: candidate.utterance_id,
        text_session_id: candidate.text_session_id,
        audio_object_key: candidate.audio_object_key,
        audio_digest: candidate.audio_digest,
        raw_transcript: candidate.raw_transcript,
        corrected_transcript: candidate.corrected_transcript,
        source_label: candidate.source_label,
        trust_score_bps: candidate.trust_score_bps,
    }
}
