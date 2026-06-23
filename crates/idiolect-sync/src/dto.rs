//! The on-the-wire data carried from phone to PC.
//!
//! [`SyncLearning`] mirrors the daemon's internal training-candidate row
//! field-for-field, but it is a *separate* serializable type on purpose: it
//! deliberately omits the train/validation/holdout `split` (the PC decides that
//! at manifest-build time) and it is free to evolve independently of the
//! storage schema.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One raw→corrected learning, ready to ship. Field-for-field the same as the
/// daemon's `ManifestV2TrainingCandidate`, minus `split`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncLearning {
    pub training_candidate_id: i64,
    pub user_id: String,
    pub utterance_id: String,
    pub text_session_id: String,
    pub audio_object_key: String,
    /// Lowercase-hex SHA-256 of the encoded audio payload. Also the key under
    /// which the audio bytes ride in [`SyncBatchEnvelope::audio`], and the ACK
    /// token the server returns once the bytes are durably stored.
    pub audio_digest: String,
    pub raw_transcript: String,
    pub corrected_transcript: String,
    pub source_label: String,
    pub trust_score_bps: u16,
}

/// A set of learnings shipped together. `batch_id` makes a re-POST idempotent;
/// `device_id` scopes dedup (`(device_id, audio_digest)`) on the server.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncBatch {
    pub device_id: String,
    pub batch_id: String,
    pub learnings: Vec<SyncLearning>,
}

/// A batch plus the audio bytes its learnings reference, content-addressed by
/// `audio_digest`. A `BTreeMap` (not a list) so a digest repeated across
/// learnings ships its bytes exactly once and encoding is deterministic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncBatchEnvelope {
    pub batch: SyncBatch,
    pub audio: BTreeMap<String, Vec<u8>>,
}

impl SyncBatchEnvelope {
    #[must_use]
    pub fn new(batch: SyncBatch, audio: BTreeMap<String, Vec<u8>>) -> Self {
        Self { batch, audio }
    }

    /// Digests referenced by a learning but missing from `audio` — a malformed
    /// envelope the server must reject rather than train on a hole.
    #[must_use]
    pub fn missing_audio_digests(&self) -> Vec<String> {
        self.batch
            .learnings
            .iter()
            .map(|learning| learning.audio_digest.clone())
            .filter(|digest| !self.audio.contains_key(digest))
            .collect()
    }
}
