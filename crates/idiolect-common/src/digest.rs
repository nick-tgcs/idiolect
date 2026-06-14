//! Content-addressed digests shared across the daemon, trainer, and (future)
//! mobile sync layer.
//!
//! The audio digest is the lowercase-hex SHA-256 of an utterance's *encoded*
//! audio payload (the IDOPUS1 container as written to the audio store). It is
//! the content-addressed key the training-manifest validation requires and the
//! sync layer will use as a dedup / ACK token, so every layer must agree on the
//! exact bytes hashed and the exact textual form.

use sha2::{Digest, Sha256};

/// Lowercase-hex SHA-256 of `payload` — the canonical content digest for an
/// utterance's encoded audio.
///
/// This is intentionally the single definition every layer calls so the daemon
/// (capture), the trainer (manifest validation), and the sync layer (dedup/ACK)
/// can never disagree on a digest for identical bytes.
#[must_use]
pub fn audio_sha256_hex(payload: &[u8]) -> String {
    let digest = Sha256::digest(payload);
    let mut output = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::audio_sha256_hex;

    #[test]
    fn matches_a_known_sha256_vector() {
        // FIPS 180-2 / RFC 6234 worked example: SHA-256("abc").
        assert_eq!(
            audio_sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
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
