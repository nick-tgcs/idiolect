//! Content-addressed digests shared across the daemon, trainer, and (future)
//! mobile sync layer.
//!
//! The audio digest is the lowercase-hex SHA-256 of an utterance's *encoded*
//! audio payload (the IDOPUS1 container as written to the audio store). It is
//! the content-addressed key the training-manifest validation requires and the
//! sync layer will use as a dedup / ACK token, so every layer must agree on the
//! exact bytes hashed and the exact textual form.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Lowercase-hex SHA-256 of `payload` — the canonical content digest for an
/// utterance's encoded audio.
///
/// This is intentionally the single definition every layer calls so the daemon
/// (capture), the trainer (manifest validation), and the sync layer (dedup/ACK)
/// can never disagree on a digest for identical bytes.
#[must_use]
pub fn audio_sha256_hex(payload: &[u8]) -> String {
    sha256_hex(payload)
}

/// Lowercase-hex SHA-256 of arbitrary bytes — the general-purpose digest the audio and
/// file helpers are specialisations of. Used by the sync server to store a device bearer
/// token only as its hash, so the on-disk token file cannot be replayed if it leaks.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

/// Lowercase-hex SHA-256 of a file's bytes, read incrementally so a multi-tens-of-MB
/// model file never sits fully in memory. Used to verify a model's integrity at
/// download/install and again at **every** load (the M5 model-management contract),
/// and by the model server to advertise the served file's digest. Agrees byte-for-byte
/// with [`audio_sha256_hex`] of the same content.
pub fn file_sha256_hex(path: impl AsRef<Path>) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Encode digest bytes as lowercase hex — the one textual form every layer agrees on.
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{audio_sha256_hex, file_sha256_hex, sha256_hex};

    #[test]
    fn matches_a_known_sha256_vector() {
        // FIPS 180-2 / RFC 6234 worked example: SHA-256("abc").
        assert_eq!(
            audio_sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_hex_matches_the_known_vector_and_the_audio_specialisation() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // The audio helper is just a domain-named alias, so they must agree byte-for-byte.
        assert_eq!(sha256_hex(b"token-xyz"), audio_sha256_hex(b"token-xyz"));
    }

    #[test]
    fn file_digest_matches_the_in_memory_digest_and_the_known_vector() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("abc.bin");
        std::fs::write(&path, b"abc").expect("write fixture");
        let digest = file_sha256_hex(&path).expect("hash file");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Streaming a file and hashing bytes in memory must agree exactly.
        assert_eq!(digest, audio_sha256_hex(b"abc"));
    }

    #[test]
    fn file_digest_spans_multiple_read_chunks() {
        // Larger than the 64 KiB read buffer, to exercise the incremental update loop.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big.bin");
        let bytes = vec![0xAB_u8; 200_000];
        std::fs::write(&path, &bytes).expect("write fixture");
        assert_eq!(
            file_sha256_hex(&path).expect("hash file"),
            audio_sha256_hex(&bytes)
        );
    }

    #[test]
    fn hashing_a_missing_file_is_an_io_error() {
        assert!(file_sha256_hex("/no/such/idiolect-file.bin").is_err());
    }

    #[test]
    fn empty_input_hashes_to_the_canonical_empty_digest() {
        assert_eq!(
            audio_sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn output_is_always_64_lowercase_hex_chars() {
        let digest = audio_sha256_hex(b"some opus payload bytes \x00\x01\xff");
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "digest must be lowercase hex: {digest}"
        );
    }
}
