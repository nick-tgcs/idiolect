//! Crate documentation for the Idiolect workspace.

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

use std::array::TryFromSliceError;

use idiolect_ports::audio::{AudioSegment, EncodedAudio};
use idiolect_ports::codec::AudioCodecPort;

const MAGIC: &[u8; 5] = b"IDFX1";
const CODEC_NAME: &str = "fixture-codec";
const F32_BYTES: usize = std::mem::size_of::<f32>();
const HEADER_BYTES: usize = 5 + 4 + 2 + 4 + 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureCodecError {
    CorruptPayload,
}

#[derive(Debug, Default)]
pub struct FixtureCodec;

impl FixtureCodec {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl AudioCodecPort for FixtureCodec {
    type Error = FixtureCodecError;

    fn encode(&self, audio: &AudioSegment) -> Result<EncodedAudio, Self::Error> {
        let sample_count = u32::try_from(audio.samples_f32_mono.len())
            .map_err(|_| FixtureCodecError::CorruptPayload)?;

        let mut payload = Vec::new();
        payload.extend_from_slice(MAGIC);
        payload.extend_from_slice(&audio.sample_rate_hz.to_le_bytes());
        payload.extend_from_slice(&audio.channels.to_le_bytes());
        payload.extend_from_slice(&audio.duration_ms.to_le_bytes());
        payload.extend_from_slice(&sample_count.to_le_bytes());

        for sample in &audio.samples_f32_mono {
            payload.extend_from_slice(&sample.to_le_bytes());
        }

        Ok(EncodedAudio {
            codec_name: CODEC_NAME.to_owned(),
            sample_rate_hz: audio.sample_rate_hz,
            channels: audio.channels,
            payload,
        })
    }

    fn decode(&self, encoded: &EncodedAudio) -> Result<AudioSegment, Self::Error> {
        if encoded.codec_name != CODEC_NAME {
            return Err(FixtureCodecError::CorruptPayload);
        }

        if encoded.payload.len() < HEADER_BYTES {
            return Err(FixtureCodecError::CorruptPayload);
        }

        if &encoded.payload[0..MAGIC.len()] != MAGIC {
            return Err(FixtureCodecError::CorruptPayload);
        }

        let mut cursor = MAGIC.len();
        let sample_rate_hz =
            u32::from_le_bytes(read_4_bytes(&encoded.payload[cursor..cursor + 4])?);
        cursor += 4;
        let channels = u16::from_le_bytes(read_2_bytes(&encoded.payload[cursor..cursor + 2])?);
        cursor += 2;
        let duration_ms = u32::from_le_bytes(read_4_bytes(&encoded.payload[cursor..cursor + 4])?);
        cursor += 4;
        let sample_count = u32::from_le_bytes(read_4_bytes(&encoded.payload[cursor..cursor + 4])?);
        cursor += 4;

        let sample_count =
            usize::try_from(sample_count).map_err(|_| FixtureCodecError::CorruptPayload)?;
        let sample_bytes = sample_count
            .checked_mul(F32_BYTES)
            .ok_or(FixtureCodecError::CorruptPayload)?;
        let expected_payload_len = HEADER_BYTES
            .checked_add(sample_bytes)
            .ok_or(FixtureCodecError::CorruptPayload)?;

        if encoded.payload.len() != expected_payload_len {
            return Err(FixtureCodecError::CorruptPayload);
        }

        let mut samples_f32_mono = Vec::with_capacity(sample_count);
        for _ in 0..sample_count {
            let sample_bytes = read_4_bytes(&encoded.payload[cursor..cursor + F32_BYTES])?;
            samples_f32_mono.push(f32::from_le_bytes(sample_bytes));
            cursor += F32_BYTES;
        }

        Ok(AudioSegment {
            sample_rate_hz,
            channels,
            duration_ms,
            samples_f32_mono,
        })
    }
}

fn read_4_bytes(bytes: &[u8]) -> Result<[u8; 4], FixtureCodecError> {
    bytes
        .try_into()
        .map_err(|_: TryFromSliceError| FixtureCodecError::CorruptPayload)
}

fn read_2_bytes(bytes: &[u8]) -> Result<[u8; 2], FixtureCodecError> {
    bytes
        .try_into()
        .map_err(|_: TryFromSliceError| FixtureCodecError::CorruptPayload)
}

#[cfg(test)]
mod tests {
    use super::{FixtureCodec, FixtureCodecError};
    use idiolect_ports::audio::{AudioSegment, EncodedAudio};
    use idiolect_ports::codec::AudioCodecPort;

    #[test]
    fn fixture_codec_round_trips_segment() {
        let original = AudioSegment {
            sample_rate_hz: 16_000,
            channels: 1,
            duration_ms: 1_000,
            samples_f32_mono: vec![0.0, 0.25, -0.5, 0.75],
        };

        let codec = FixtureCodec::new();
        let encoded = codec.encode(&original).expect("encoding should succeed");
        let decoded = codec.decode(&encoded).expect("decoding should succeed");

        assert_eq!(encoded.codec_name, "fixture-codec");
        assert_eq!(decoded.sample_rate_hz, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.duration_ms, 1_000);
        assert_eq!(decoded.samples_f32_mono, original.samples_f32_mono);
    }

    #[test]
    fn fixture_codec_rejects_corrupt_payload() {
        let original = AudioSegment {
            sample_rate_hz: 16_000,
            channels: 1,
            duration_ms: 1_000,
            samples_f32_mono: vec![0.0, 0.5, -0.5],
        };

        let codec = FixtureCodec::new();
        let mut encoded = codec.encode(&original).expect("encoding should succeed");
        encoded.payload.truncate(encoded.payload.len() - 1);

        assert_eq!(
            codec.decode(&encoded),
            Err(FixtureCodecError::CorruptPayload)
        );
    }

    #[test]
    fn fixture_codec_rejects_wrong_codec_name() {
        let codec = FixtureCodec::new();
        let corrupt = EncodedAudio {
            codec_name: "not-fixture-codec".to_owned(),
            sample_rate_hz: 16_000,
            channels: 1,
            payload: vec![0; 32],
        };

        assert_eq!(
            codec.decode(&corrupt),
            Err(FixtureCodecError::CorruptPayload)
        );
    }
}
