//! A small, self-describing, length-prefixed binary container for a
//! [`SyncBatchEnvelope`].
//!
//! Why not `multipart/mixed`? Both ends of this protocol are ours, the payload
//! is binary audio, and a length-prefixed frame is far more robust than
//! boundary-scanning binary blobs (no boundary-collision class of bug, no
//! base64 bloat). At the HTTP layer (S2) this is simply the request body
//! (`application/vnd.idiolect.sync.v1`). The layout:
//!
//! ```text
//! "IDSYNC1"                      7-byte magic
//! u32 LE  json_len               length of the SyncBatch JSON
//! [u8]    json                    serde_json of the SyncBatch (no audio)
//! u32 LE  audio_count            number of content-addressed audio parts
//! repeat audio_count:
//!   u32 LE digest_len, [u8] digest   (utf-8 lowercase-hex SHA-256)
//!   u32 LE blob_len,   [u8] blob     (the encoded IDOPUS1 payload)
//! ```
//!
//! All lengths are `u32` little-endian; audio parts are emitted in the
//! envelope's `BTreeMap` order, so encoding is deterministic.

use std::collections::BTreeMap;

use crate::dto::{SyncBatch, SyncBatchEnvelope};

const MAGIC: &[u8; 7] = b"IDSYNC1";

/// Failure modes when encoding or decoding a sync container.
#[derive(Debug, thiserror::Error)]
pub enum SyncCodecError {
    #[error("bad sync container magic")]
    BadMagic,
    #[error("unexpected end of sync container")]
    UnexpectedEof,
    #[error("trailing bytes after sync container")]
    TrailingBytes,
    #[error("duplicate audio digest in sync container")]
    DuplicateAudioDigest,
    #[error("audio digest was not valid utf-8")]
    InvalidDigestUtf8,
    #[error("a chunk was too large to encode")]
    ChunkTooLarge,
    #[error("too many audio parts to encode")]
    TooManyParts,
    #[error("sync batch json error: {0}")]
    Json(#[source] serde_json::Error),
}

/// Serialize an envelope into its wire bytes.
pub fn encode_batch(envelope: &SyncBatchEnvelope) -> Result<Vec<u8>, SyncCodecError> {
    let json = serde_json::to_vec(&envelope.batch).map_err(SyncCodecError::Json)?;
    let mut out = Vec::with_capacity(MAGIC.len() + 8 + json.len());
    out.extend_from_slice(MAGIC);
    write_chunk(&mut out, &json)?;

    let count = u32::try_from(envelope.audio.len()).map_err(|_| SyncCodecError::TooManyParts)?;
    out.extend_from_slice(&count.to_le_bytes());
    for (digest, blob) in &envelope.audio {
        write_chunk(&mut out, digest.as_bytes())?;
        write_chunk(&mut out, blob)?;
    }
    Ok(out)
}

/// Parse wire bytes back into an envelope, rejecting anything malformed.
pub fn decode_batch(bytes: &[u8]) -> Result<SyncBatchEnvelope, SyncCodecError> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(MAGIC.len())? != &MAGIC[..] {
        return Err(SyncCodecError::BadMagic);
    }

    let batch: SyncBatch =
        serde_json::from_slice(cursor.take_chunk()?).map_err(SyncCodecError::Json)?;

    let count = cursor.take_u32()?;
    let mut audio = BTreeMap::new();
    for _ in 0..count {
        let digest = std::str::from_utf8(cursor.take_chunk()?)
            .map_err(|_| SyncCodecError::InvalidDigestUtf8)?
            .to_owned();
        let blob = cursor.take_chunk()?.to_vec();
        if audio.insert(digest, blob).is_some() {
            return Err(SyncCodecError::DuplicateAudioDigest);
        }
    }

    if !cursor.is_empty() {
        return Err(SyncCodecError::TrailingBytes);
    }
    Ok(SyncBatchEnvelope { batch, audio })
}

fn write_chunk(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), SyncCodecError> {
    let len = u32::try_from(bytes.len()).map_err(|_| SyncCodecError::ChunkTooLarge)?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], SyncCodecError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(SyncCodecError::UnexpectedEof)?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(SyncCodecError::UnexpectedEof)?;
        self.pos = end;
        Ok(slice)
    }

    fn take_u32(&mut self) -> Result<u32, SyncCodecError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn take_chunk(&mut self) -> Result<&'a [u8], SyncCodecError> {
        let len = self.take_u32()? as usize;
        self.take(len)
    }
}
