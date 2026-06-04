//! Opus codec adapter for Idiolect.

use idiolect_ports::audio::{AudioSegment, EncodedAudio};
use idiolect_ports::codec::AudioCodecPort;

const CODEC_NAME: &str = "opus";
const SAMPLE_RATE_HZ: u32 = 16_000;
const CHANNELS: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpusCodecError {
    UnsupportedSampleRate {
        sample_rate_hz: u32,
    },
    UnsupportedChannelCount {
        channels: u16,
    },
    UnsupportedCodecName {
        codec_name: String,
    },
    CorruptPayload,
    Backend {
        function: &'static str,
        description: &'static str,
    },
}

impl std::fmt::Display for OpusCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSampleRate { sample_rate_hz } => {
                write!(f, "unsupported sample rate: {sample_rate_hz}")
            }
            Self::UnsupportedChannelCount { channels } => {
                write!(f, "unsupported channel count: {channels}")
            }
            Self::UnsupportedCodecName { codec_name } => {
                write!(f, "unsupported codec name: {codec_name}")
            }
            Self::CorruptPayload => f.write_str("corrupt opus payload"),
            Self::Backend {
                function,
                description,
            } => {
                write!(f, "{function}: {description}")
            }
        }
    }
}

impl std::error::Error for OpusCodecError {}

#[derive(Debug, Default, Clone, Copy)]
pub struct OpusCodec;

impl OpusCodec {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl AudioCodecPort for OpusCodec {
    type Error = OpusCodecError;

    fn encode(&self, audio: &AudioSegment) -> Result<EncodedAudio, Self::Error> {
        backend::encode(audio)
    }

    fn decode(&self, encoded: &EncodedAudio) -> Result<AudioSegment, Self::Error> {
        backend::decode(encoded)
    }
}

mod backend {
    use super::{AudioSegment, EncodedAudio, OpusCodecError, CHANNELS, CODEC_NAME, SAMPLE_RATE_HZ};
    use opus::{Application, Channels, Decoder, Encoder};
    use std::convert::TryFrom;

    const FRAME_SAMPLES: usize = 320;
    const MAX_PACKET_BYTES: usize = 4_096;
    const MAGIC: &[u8; 7] = b"IDOPUS1";
    const HEADER_BYTES: usize = MAGIC.len() + 4 + 4 + 4;

    pub(crate) fn encode(audio: &AudioSegment) -> Result<EncodedAudio, OpusCodecError> {
        validate_audio(audio.sample_rate_hz, audio.channels)?;

        let sample_count = u32::try_from(audio.samples_f32_mono.len())
            .map_err(|_| OpusCodecError::CorruptPayload)?;
        let packet_count = if audio.samples_f32_mono.is_empty() {
            0usize
        } else {
            audio.samples_f32_mono.chunks(FRAME_SAMPLES).count()
        };

        let mut payload = Vec::new();
        payload.extend_from_slice(MAGIC);
        payload.extend_from_slice(&audio.duration_ms.to_le_bytes());
        payload.extend_from_slice(&sample_count.to_le_bytes());
        payload.extend_from_slice(
            &u32::try_from(packet_count)
                .map_err(|_| OpusCodecError::CorruptPayload)?
                .to_le_bytes(),
        );

        if audio.samples_f32_mono.is_empty() {
            return Ok(EncodedAudio {
                codec_name: CODEC_NAME.to_owned(),
                sample_rate_hz: audio.sample_rate_hz,
                channels: audio.channels,
                payload,
            });
        }

        let mut encoder = Encoder::new(SAMPLE_RATE_HZ, Channels::Mono, Application::Audio)
            .map_err(map_backend_error)?;

        for chunk in audio.samples_f32_mono.chunks(FRAME_SAMPLES) {
            let mut frame = [0.0f32; FRAME_SAMPLES];
            frame[..chunk.len()].copy_from_slice(chunk);

            let packet = encoder
                .encode_vec_float(&frame, MAX_PACKET_BYTES)
                .map_err(map_backend_error)?;
            let packet_len =
                u32::try_from(packet.len()).map_err(|_| OpusCodecError::CorruptPayload)?;
            payload.extend_from_slice(&packet_len.to_le_bytes());
            payload.extend_from_slice(&packet);
        }

        Ok(EncodedAudio {
            codec_name: CODEC_NAME.to_owned(),
            sample_rate_hz: audio.sample_rate_hz,
            channels: audio.channels,
            payload,
        })
    }

    pub(crate) fn decode(encoded: &EncodedAudio) -> Result<AudioSegment, OpusCodecError> {
        if encoded.codec_name != CODEC_NAME {
            return Err(OpusCodecError::UnsupportedCodecName {
                codec_name: encoded.codec_name.clone(),
            });
        }

        validate_audio(encoded.sample_rate_hz, encoded.channels)?;

        let parsed = parse_payload(&encoded.payload)?;
        let mut decoder =
            Decoder::new(SAMPLE_RATE_HZ, Channels::Mono).map_err(map_backend_error)?;
        let mut samples = Vec::with_capacity(parsed.sample_count as usize);

        for packet in parsed.packets {
            let mut frame = [0.0f32; FRAME_SAMPLES];
            let decoded = decoder
                .decode_float(&packet, &mut frame, false)
                .map_err(map_backend_error)?;
            samples.extend_from_slice(&frame[..decoded]);
        }

        if samples.len() < parsed.sample_count as usize {
            return Err(OpusCodecError::CorruptPayload);
        }

        samples.truncate(parsed.sample_count as usize);

        Ok(AudioSegment {
            sample_rate_hz: encoded.sample_rate_hz,
            channels: encoded.channels,
            duration_ms: parsed.duration_ms,
            samples_f32_mono: samples,
        })
    }

    fn validate_audio(sample_rate_hz: u32, channels: u16) -> Result<(), OpusCodecError> {
        if sample_rate_hz != SAMPLE_RATE_HZ {
            return Err(OpusCodecError::UnsupportedSampleRate { sample_rate_hz });
        }

        if channels != CHANNELS {
            return Err(OpusCodecError::UnsupportedChannelCount { channels });
        }

        Ok(())
    }

    struct ParsedPayload {
        duration_ms: u32,
        sample_count: u32,
        packets: Vec<Vec<u8>>,
    }

    fn parse_payload(payload: &[u8]) -> Result<ParsedPayload, OpusCodecError> {
        if payload.len() < HEADER_BYTES || &payload[..MAGIC.len()] != MAGIC {
            return Err(OpusCodecError::CorruptPayload);
        }

        let mut cursor = MAGIC.len();
        let duration_ms = read_u32(payload, &mut cursor)?;
        let sample_count = read_u32(payload, &mut cursor)?;
        let packet_count = read_u32(payload, &mut cursor)? as usize;

        let mut packets = Vec::with_capacity(packet_count);
        for _ in 0..packet_count {
            let packet_len = read_u32(payload, &mut cursor)? as usize;
            let end = cursor
                .checked_add(packet_len)
                .ok_or(OpusCodecError::CorruptPayload)?;
            if end > payload.len() {
                return Err(OpusCodecError::CorruptPayload);
            }
            packets.push(payload[cursor..end].to_vec());
            cursor = end;
        }

        if cursor != payload.len() {
            return Err(OpusCodecError::CorruptPayload);
        }

        Ok(ParsedPayload {
            duration_ms,
            sample_count,
            packets,
        })
    }

    fn read_u32(payload: &[u8], cursor: &mut usize) -> Result<u32, OpusCodecError> {
        let end = cursor
            .checked_add(4)
            .ok_or(OpusCodecError::CorruptPayload)?;
        if end > payload.len() {
            return Err(OpusCodecError::CorruptPayload);
        }

        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(&payload[*cursor..end]);
        *cursor = end;
        Ok(u32::from_le_bytes(bytes))
    }

    fn map_backend_error(error: opus::Error) -> OpusCodecError {
        OpusCodecError::Backend {
            function: error.function(),
            description: error.description(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OpusCodec, OpusCodecError, CHANNELS, CODEC_NAME, SAMPLE_RATE_HZ};
    use idiolect_ports::audio::{AudioSegment, EncodedAudio};
    use idiolect_ports::codec::AudioCodecPort;
    use idiolect_test_support::fixtures::sine_fixture_16khz_mono;

    #[test]
    fn unsupported_sample_rate_returns_typed_error() {
        let codec = OpusCodec::new();
        let audio = AudioSegment {
            sample_rate_hz: 44_100,
            channels: 1,
            duration_ms: 1,
            samples_f32_mono: vec![0.0; 4],
        };

        assert_eq!(
            codec.encode(&audio),
            Err(OpusCodecError::UnsupportedSampleRate {
                sample_rate_hz: 44_100,
            })
        );
    }

    #[test]
    fn opus_codec_round_trips_fixture_metadata() {
        let codec = OpusCodec::new();
        let fixture = sine_fixture_16khz_mono();

        let encoded = codec.encode(&fixture).expect("fixture should encode");
        let decoded = codec.decode(&encoded).expect("fixture should decode");

        assert_eq!(encoded.codec_name, CODEC_NAME);
        assert_eq!(decoded.sample_rate_hz, fixture.sample_rate_hz);
        assert_eq!(decoded.channels, fixture.channels);
        assert_eq!(decoded.duration_ms, fixture.duration_ms);
        assert_eq!(decoded.sample_count(), fixture.sample_count());
        assert_eq!(decoded.sample_rate_hz, SAMPLE_RATE_HZ);
        assert_eq!(decoded.channels, CHANNELS);
    }

    #[test]
    fn decode_rejects_non_opus_codec_name() {
        let codec = OpusCodec::new();
        let encoded = EncodedAudio {
            codec_name: "fixture-codec".to_owned(),
            sample_rate_hz: SAMPLE_RATE_HZ,
            channels: CHANNELS,
            payload: vec![],
        };

        assert_eq!(
            codec.decode(&encoded),
            Err(OpusCodecError::UnsupportedCodecName {
                codec_name: "fixture-codec".to_owned(),
            })
        );
    }
}
