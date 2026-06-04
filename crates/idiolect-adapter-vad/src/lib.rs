//! Idiolect VAD adapter.

use std::fmt;

use idiolect_ports::audio::AudioSegment;
use idiolect_ports::vad::VadPort;
use webrtc_vad::{SampleRate, Vad, VadMode};

const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;
const TARGET_CHANNELS: u16 = 1;
const FRAME_DURATION_MS: usize = 30;
const FRAME_SAMPLE_COUNT: usize = (TARGET_SAMPLE_RATE_HZ as usize * FRAME_DURATION_MS) / 1_000;
const MAX_MERGE_GAP_MS: usize = 300;
const MAX_MERGE_GAP_FRAMES: usize = MAX_MERGE_GAP_MS / FRAME_DURATION_MS;

/// VAD adapter that keeps the detector private.
pub struct VadAdapter {
    detector: Vad,
}

/// Typed adapter errors for invalid audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadAdapterError {
    UnsupportedSampleRate { expected: u32, got: u32 },
    UnsupportedChannels { expected: u16, got: u16 },
    BackendFrameRejected,
}

impl fmt::Display for VadAdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSampleRate { expected, got } => {
                write!(
                    f,
                    "unsupported sample rate: expected {expected} Hz, got {got} Hz"
                )
            }
            Self::UnsupportedChannels { expected, got } => {
                write!(
                    f,
                    "unsupported channel count: expected {expected}, got {got}"
                )
            }
            Self::BackendFrameRejected => write!(f, "VAD backend rejected a fixed-size frame"),
        }
    }
}

impl std::error::Error for VadAdapterError {}

impl VadAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            detector: new_detector(),
        }
    }
}

impl Default for VadAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl VadPort for VadAdapter {
    type Error = VadAdapterError;

    fn segment(&mut self, audio: &AudioSegment) -> Result<Vec<AudioSegment>, Self::Error> {
        validate_audio(audio)?;
        self.detector = new_detector();

        let samples = samples_to_i16(&audio.samples_f32_mono);
        let frame_labels = samples
            .chunks_exact(FRAME_SAMPLE_COUNT)
            .map(|frame| {
                self.detector
                    .is_voice_segment(frame)
                    .map_err(|()| VadAdapterError::BackendFrameRejected)
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(group_contiguous_speech_regions(audio, &frame_labels))
    }
}

fn new_detector() -> Vad {
    Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VadMode::Quality)
}

fn validate_audio(audio: &AudioSegment) -> Result<(), VadAdapterError> {
    if audio.sample_rate_hz != TARGET_SAMPLE_RATE_HZ {
        return Err(VadAdapterError::UnsupportedSampleRate {
            expected: TARGET_SAMPLE_RATE_HZ,
            got: audio.sample_rate_hz,
        });
    }

    if audio.channels != TARGET_CHANNELS {
        return Err(VadAdapterError::UnsupportedChannels {
            expected: TARGET_CHANNELS,
            got: audio.channels,
        });
    }

    Ok(())
}

fn samples_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .copied()
        .map(|sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
        .collect()
}

fn group_contiguous_speech_regions(
    audio: &AudioSegment,
    frame_labels: &[bool],
) -> Vec<AudioSegment> {
    let mut ranges = Vec::new();
    let mut start_frame = None;

    for (frame_index, is_speech) in frame_labels.iter().copied().enumerate() {
        match (start_frame, is_speech) {
            (None, true) => start_frame = Some(frame_index),
            (Some(begin), false) => {
                ranges.push((begin, frame_index));
                start_frame = None;
            }
            _ => {}
        }
    }

    if let Some(begin) = start_frame {
        ranges.push((begin, frame_labels.len()));
    }

    let merged_ranges = merge_short_gaps(ranges);
    merged_ranges
        .into_iter()
        .map(|(begin, end)| slice_audio_segment_by_frame(audio, begin, end))
        .collect()
}

fn merge_short_gaps(ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::new();

    for (begin, end) in ranges {
        if let Some((_, previous_end)) = merged.last_mut() {
            if begin.saturating_sub(*previous_end) <= MAX_MERGE_GAP_FRAMES {
                *previous_end = end;
                continue;
            }
        }

        merged.push((begin, end));
    }

    merged
}

fn slice_audio_segment_by_frame(
    audio: &AudioSegment,
    start_frame: usize,
    end_frame: usize,
) -> AudioSegment {
    let start = start_frame * FRAME_SAMPLE_COUNT;
    let end = (end_frame * FRAME_SAMPLE_COUNT).min(audio.samples_f32_mono.len());
    let samples_f32_mono = audio.samples_f32_mono[start..end].to_vec();
    let duration_ms = ((samples_f32_mono.len() as u64) * 1_000 / (audio.sample_rate_hz as u64))
        .try_into()
        .expect("segment duration should fit in u32");

    AudioSegment {
        sample_rate_hz: audio.sample_rate_hz,
        channels: audio.channels,
        duration_ms,
        samples_f32_mono,
    }
}

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    use super::{VadAdapter, VadAdapterError};
    use idiolect_ports::audio::AudioSegment;
    use idiolect_ports::vad::VadPort;
    use idiolect_test_support::fixtures::speech_and_silence_fixture_16khz_mono;

    #[test]
    fn vad_segments_fixture_into_speech_regions() {
        let mut adapter = VadAdapter::new();
        let fixture = speech_and_silence_fixture_16khz_mono();

        let segments = adapter
            .segment(&fixture)
            .expect("fixture should segment into speech");

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].sample_rate_hz, 16_000);
        assert!(segments[0].duration_ms >= 400);
    }

    #[test]
    fn vad_reports_typed_errors_for_bad_audio() {
        let mut adapter = VadAdapter::new();
        let bad_audio = AudioSegment {
            sample_rate_hz: 8_000,
            channels: 1,
            duration_ms: 1_000,
            samples_f32_mono: vec![0.0; 8_000],
        };

        assert_eq!(
            adapter.segment(&bad_audio),
            Err(VadAdapterError::UnsupportedSampleRate {
                expected: 16_000,
                got: 8_000,
            })
        );
    }

    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
