use std::f32::consts::PI;

use idiolect_ports::audio::AudioSegment;

const RESTART_TRAFFIC_WAV: &[u8] =
    include_bytes!("../../../tests/fixtures/audio/restart_traffic_16khz_mono.wav");
const SILENCE_PADDING_MS: u32 = 250;

/// Creates a deterministic 16 kHz, one-second, mono sine-wave fixture.
pub fn sine_fixture_16khz_mono() -> AudioSegment {
    let sample_rate_hz = 16_000u32;
    let duration_ms = 1_000u32;
    let channels = 1u16;
    let frequency_hz = 440.0f32;

    let samples_f32_mono = (0..sample_rate_hz)
        .map(|sample_index| {
            let phase = 2.0 * PI * frequency_hz * (sample_index as f32) / (sample_rate_hz as f32);
            phase.sin()
        })
        .collect::<Vec<f32>>();

    AudioSegment {
        sample_rate_hz,
        channels,
        duration_ms,
        samples_f32_mono,
    }
}

/// Loads the committed restart-traffic WAV fixture as mono f32 samples.
pub fn restart_traffic_fixture_16khz_mono() -> AudioSegment {
    load_wav_fixture(RESTART_TRAFFIC_WAV)
}

/// Returns the restart-traffic clip with deterministic silence padded around it.
pub fn speech_and_silence_fixture_16khz_mono() -> AudioSegment {
    pad_with_silence(&restart_traffic_fixture_16khz_mono(), SILENCE_PADDING_MS)
}

/// Two utterances separated (and followed) by a clear pause: speech, ≥1.25 s of
/// silence, the same speech again, ≥1 s of trailing silence. Drives the
/// pause-triggered segmentation path: a correct segmenter emits exactly two
/// snippets from this clip, both before the recording stops. The pauses leave
/// generous headroom over the default 700 ms threshold because the WebRTC VAD's
/// speech hangover spills ~100 ms of "speech" labels into the silence.
pub fn speech_pause_speech_fixture_16khz_mono() -> AudioSegment {
    let utterance = speech_and_silence_fixture_16khz_mono();
    let gap = vec![0.0_f32; 12_000]; // 750 ms at 16 kHz, on top of the 2×250 ms pads

    let mut samples_f32_mono = utterance.samples_f32_mono.clone();
    samples_f32_mono.extend_from_slice(&gap);
    samples_f32_mono.extend_from_slice(&utterance.samples_f32_mono);
    samples_f32_mono.extend_from_slice(&gap);

    let duration_ms = ((samples_f32_mono.len() as u64) * 1_000 / 16_000) as u32;
    AudioSegment {
        sample_rate_hz: 16_000,
        channels: 1,
        duration_ms,
        samples_f32_mono,
    }
}

fn pad_with_silence(audio: &AudioSegment, padding_ms: u32) -> AudioSegment {
    let pad_samples = (audio.sample_rate_hz as usize * padding_ms as usize) / 1_000usize;

    let mut samples_f32_mono = vec![0.0; pad_samples];
    samples_f32_mono.extend_from_slice(&audio.samples_f32_mono);
    samples_f32_mono.extend(std::iter::repeat_n(0.0, pad_samples));

    AudioSegment {
        sample_rate_hz: audio.sample_rate_hz,
        channels: audio.channels,
        duration_ms: audio.duration_ms + padding_ms * 2,
        samples_f32_mono,
    }
}

fn load_wav_fixture(bytes: &[u8]) -> AudioSegment {
    assert!(bytes.len() >= 12, "fixture wav is too short");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");

    let mut cursor = 12usize;
    let mut audio_format = None;
    let mut channels = None;
    let mut sample_rate_hz = None;
    let mut bits_per_sample = None;
    let mut data = None;

    while cursor + 8 <= bytes.len() {
        let chunk_id = &bytes[cursor..cursor + 4];
        let chunk_size = read_u32_le(bytes, cursor + 4) as usize;
        let chunk_start = cursor + 8;
        let chunk_end = chunk_start + chunk_size;
        assert!(chunk_end <= bytes.len(), "fixture wav chunk is truncated");

        if chunk_id == b"fmt " {
            assert!(chunk_size >= 16, "fixture wav fmt chunk is too small");
            audio_format = Some(read_u16_le(bytes, chunk_start));
            channels = Some(read_u16_le(bytes, chunk_start + 2));
            sample_rate_hz = Some(read_u32_le(bytes, chunk_start + 4));
            bits_per_sample = Some(read_u16_le(bytes, chunk_start + 14));
        } else if chunk_id == b"data" {
            data = Some(&bytes[chunk_start..chunk_end]);
        }

        cursor = chunk_end + (chunk_size % 2);
    }

    let audio_format = audio_format.expect("fixture wav is missing fmt chunk");
    let channels = channels.expect("fixture wav is missing channel metadata");
    let sample_rate_hz = sample_rate_hz.expect("fixture wav is missing sample rate");
    let bits_per_sample = bits_per_sample.expect("fixture wav is missing bit depth");
    let data = data.expect("fixture wav is missing data chunk");

    assert_eq!(audio_format, 1, "fixture wav must be PCM");
    assert_eq!(channels, 1, "fixture wav must be mono");
    assert_eq!(sample_rate_hz, 16_000, "fixture wav must stay at 16 kHz");
    assert_eq!(bits_per_sample, 16, "fixture wav must be PCM s16le");
    assert_eq!(data.len() % 2, 0, "PCM16 fixture data must be aligned");

    let samples_f32_mono = data
        .chunks_exact(2)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0)
        .collect::<Vec<f32>>();
    let duration_ms = ((samples_f32_mono.len() as u64) * 1_000u64 / (sample_rate_hz as u64)) as u32;

    AudioSegment {
        sample_rate_hz,
        channels,
        duration_ms,
        samples_f32_mono,
    }
}

fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::{
        restart_traffic_fixture_16khz_mono, sine_fixture_16khz_mono,
        speech_and_silence_fixture_16khz_mono,
    };

    #[test]
    fn sine_fixture_has_expected_shape() {
        let segment = sine_fixture_16khz_mono();

        assert_eq!(segment.sample_rate_hz, 16_000);
        assert_eq!(segment.channels, 1);
        assert_eq!(segment.duration_ms, 1_000);
        assert_eq!(segment.sample_count(), 16_000);
        assert_eq!(segment.samples_f32_mono[0], 0.0);
    }

    #[test]
    fn sine_fixture_is_deterministic() {
        let first = sine_fixture_16khz_mono();
        let second = sine_fixture_16khz_mono();

        assert_eq!(first.samples_f32_mono, second.samples_f32_mono);
    }

    #[test]
    fn restart_traffic_fixture_has_expected_shape() {
        let first = restart_traffic_fixture_16khz_mono();
        let second = restart_traffic_fixture_16khz_mono();

        assert_eq!(first, second);
        assert_eq!(first.sample_rate_hz, 16_000);
        assert_eq!(first.channels, 1);
        assert!(first.duration_ms > 0);
        assert!(!first.samples_f32_mono.is_empty());
        assert_eq!(first.sample_count(), first.samples_f32_mono.len());
    }

    #[test]
    fn speech_pause_speech_fixture_doubles_the_utterance_with_gaps() {
        let utterance = super::speech_and_silence_fixture_16khz_mono();
        let clip = super::speech_pause_speech_fixture_16khz_mono();

        assert_eq!(clip.sample_rate_hz, 16_000);
        assert_eq!(clip.channels, 1);
        assert_eq!(
            clip.samples_f32_mono.len(),
            2 * utterance.samples_f32_mono.len() + 2 * 12_000
        );
        assert_eq!(clip, super::speech_pause_speech_fixture_16khz_mono());
    }

    #[test]
    fn speech_and_silence_fixture_has_expected_padding() {
        let restart_traffic = restart_traffic_fixture_16khz_mono();
        let speech_and_silence = speech_and_silence_fixture_16khz_mono();

        assert_eq!(speech_and_silence.sample_rate_hz, 16_000);
        assert_eq!(speech_and_silence.channels, 1);
        assert_eq!(
            speech_and_silence.duration_ms,
            restart_traffic.duration_ms + 500
        );
        assert_eq!(
            speech_and_silence.samples_f32_mono.len(),
            restart_traffic.samples_f32_mono.len() + 8_000
        );
        assert_eq!(speech_and_silence.samples_f32_mono.first(), Some(&0.0));
        assert_eq!(speech_and_silence.samples_f32_mono.last(), Some(&0.0));
        assert_eq!(speech_and_silence, speech_and_silence_fixture_16khz_mono());
    }
}
