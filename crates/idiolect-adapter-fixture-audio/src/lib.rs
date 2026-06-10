//! Crate documentation for the Idiolect workspace.

use std::collections::BTreeSet;

use idiolect_common::ids::ImeSessionId;
use idiolect_ports::audio::{AudioInputPort, AudioSegment};
use idiolect_test_support::fixtures::sine_fixture_16khz_mono;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureAudioError {
    NotStarted,
}

#[derive(Debug, Default)]
pub struct FixtureAudio {
    started_sessions: BTreeSet<ImeSessionId>,
}

impl FixtureAudio {
    #[must_use]
    pub fn new() -> Self {
        Self {
            started_sessions: BTreeSet::new(),
        }
    }
}

impl AudioInputPort for FixtureAudio {
    type Error = FixtureAudioError;

    fn start_capture(&mut self, session_id: ImeSessionId) -> Result<(), Self::Error> {
        self.started_sessions.insert(session_id);
        Ok(())
    }

    fn stop_capture(&mut self, session_id: ImeSessionId) -> Result<AudioSegment, Self::Error> {
        if self.started_sessions.remove(&session_id) {
            Ok(sine_fixture_16khz_mono())
        } else {
            Err(FixtureAudioError::NotStarted)
        }
    }

    fn poll_captured(&mut self, session_id: ImeSessionId) -> Result<AudioSegment, Self::Error> {
        if !self.started_sessions.contains(&session_id) {
            return Err(FixtureAudioError::NotStarted);
        }
        // The deterministic clip materialises only on stop; mid-capture polls
        // observe an empty (still-running) stream.
        Ok(AudioSegment {
            sample_rate_hz: 16_000,
            channels: 1,
            duration_ms: 0,
            samples_f32_mono: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_audio_uses_typed_session_set() {
        let fixture_audio = FixtureAudio::new();

        fn assert_typed_set(_: &BTreeSet<ImeSessionId>) {}
        assert_typed_set(&fixture_audio.started_sessions);
    }

    #[test]
    fn fixture_audio_requires_start_before_stop() {
        let mut fixture_audio = FixtureAudio::new();
        let session_id = ImeSessionId::new();

        let captured = fixture_audio.stop_capture(session_id);
        assert_eq!(captured, Err(FixtureAudioError::NotStarted));
    }

    #[test]
    fn fixture_audio_poll_requires_start_and_is_empty_mid_capture() {
        let mut fixture_audio = FixtureAudio::new();
        let session_id = ImeSessionId::new();

        assert_eq!(
            fixture_audio.poll_captured(session_id),
            Err(FixtureAudioError::NotStarted)
        );

        fixture_audio
            .start_capture(session_id)
            .expect("fixture audio should start capture");
        let polled = fixture_audio
            .poll_captured(session_id)
            .expect("poll should succeed mid-capture");
        assert!(polled.samples_f32_mono.is_empty());

        // Polling must not consume the stop-time clip.
        let segment = fixture_audio
            .stop_capture(session_id)
            .expect("stop after poll");
        assert_eq!(segment.samples_f32_mono.len(), 16_000);
    }

    #[test]
    fn fixture_audio_stop_returns_fixture_segment_after_start() {
        let mut fixture_audio = FixtureAudio::new();
        let session_id = ImeSessionId::new();

        fixture_audio
            .start_capture(session_id)
            .expect("fixture audio should start capture");

        let segment = fixture_audio
            .stop_capture(session_id)
            .expect("fixture audio should stop capture after start");

        assert_eq!(segment.sample_rate_hz, 16_000);
        assert_eq!(segment.channels, 1);
        assert_eq!(segment.samples_f32_mono.len(), 16_000);
    }
}
