//! Crate documentation for the Idiolect workspace.

use std::convert::Infallible;

use idiolect_ports::asr::{AdapterCapabilities, AsrPort, TranscriptDraft, TranscriptMetadata};
use idiolect_ports::audio::AudioSegment;

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

/// Deterministic fixture implementation of [`AsrPort`] used for testing.
pub struct FixtureAsr {
    transcript: String,
}

impl FixtureAsr {
    pub fn new<S: Into<String>>(transcript: S) -> Self {
        Self {
            transcript: transcript.into(),
        }
    }
}

impl AsrPort for FixtureAsr {
    type Error = Infallible;

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            name: "fixture-asr".to_owned(),
            version: "0.1.0".to_owned(),
            supports_streaming: false,
            supports_word_timestamps: false,
            supports_confidence: true,
            supports_gpu: false,
            supports_incremental_updates: false,
        }
    }

    fn transcribe(&self, _audio: &AudioSegment) -> Result<TranscriptDraft, Self::Error> {
        Ok(TranscriptDraft {
            text: self.transcript.clone(),
            metadata: TranscriptMetadata {
                engine_name: "fixture-asr".to_owned(),
                engine_version: "0.1.0".to_owned(),
                confidence: Some(1.0),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::FixtureAsr;
    use idiolect_ports::asr::AsrPort;
    use idiolect_test_support::fixtures::sine_fixture_16khz_mono;

    #[test]
    fn fixture_asr_returns_configured_transcript_and_metadata() {
        let adapter = FixtureAsr::new("restart traffic");
        let sine_audio = sine_fixture_16khz_mono();

        let draft = adapter
            .transcribe(&sine_audio)
            .expect("fixture asr should always transcribe");

        assert_eq!(draft.text, "restart traffic");
        assert_eq!(draft.metadata.engine_name, "fixture-asr");
        assert_eq!(draft.metadata.engine_version, "0.1.0");
        assert_eq!(draft.metadata.confidence, Some(1.0));
    }

    #[test]
    fn fixture_asr_reports_capabilities() {
        let adapter = FixtureAsr::new("restart traffic");
        let capabilities = adapter.capabilities();

        assert_eq!(capabilities.name, "fixture-asr");
        assert_eq!(capabilities.version, "0.1.0");
        assert!(!capabilities.supports_streaming);
        assert!(!capabilities.supports_word_timestamps);
        assert!(capabilities.supports_confidence);
        assert!(!capabilities.supports_gpu);
        assert!(!capabilities.supports_incremental_updates);
    }

    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
