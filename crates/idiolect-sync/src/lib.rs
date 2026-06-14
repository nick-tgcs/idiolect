//! Wire types and the on-the-wire codec for shipping on-device "learnings"
//! (raw→corrected pairs plus their audio) from the phone to the PC.
//!
//! This crate is a leaf: it depends only on serde and knows nothing about
//! storage, HTTP, or the trainer. The daemon/adapters convert their internal
//! candidate rows into [`dto::SyncLearning`]s; the sync client batches and
//! encodes them; the PC server decodes and feeds them to the existing trainer.
//!
//! The audio for each learning is carried *content-addressed* by its
//! `audio_digest` (lowercase-hex SHA-256 of the encoded payload — the same
//! digest the capture path now stores and the manifest validates), so a batch
//! that repeats a digest ships the bytes once and the server dedups for free.

pub mod codec;
pub mod dto;

pub use codec::{decode_batch, encode_batch, SyncCodecError};
pub use dto::{SyncBatch, SyncBatchEnvelope, SyncLearning};
