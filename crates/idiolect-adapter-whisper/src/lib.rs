#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use idiolect_ports::asr::{AdapterCapabilities, AsrPort, TranscriptDraft, TranscriptMetadata};
use idiolect_ports::audio::AudioSegment;
use thiserror::Error;

const ENGINE_NAME: &str = "whisper-rs";
const WHISPER_RS_VERSION: &str = "0.16.0";
const PRIMARY_FIXTURE_MODEL_FILE: &str = "ggml-tiny.en.bin";

pub struct WhisperAsr {
    backend: backend::WhisperBackend,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("missing Whisper fixture model at {path}")]
pub struct MissingFixtureModel {
    path: PathBuf,
}

impl MissingFixtureModel {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WhisperAsrLoadError {
    #[error(transparent)]
    MissingFixtureModel(#[from] MissingFixtureModel),
    #[error(transparent)]
    Backend(#[from] WhisperAsrError),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("whisper backend error: {message}")]
pub struct WhisperAsrError {
    message: String,
}

impl WhisperAsrError {
    fn backend(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

mod backend {
    use super::*;
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    #[derive(Debug)]
    pub(crate) struct WhisperBackend {
        context: WhisperContext,
    }

    impl WhisperBackend {
        pub(crate) fn load_fixture_model() -> Result<Self, WhisperAsrLoadError> {
            Self::load_model_from_path(fixture_model_path())
        }

        pub(crate) fn load_model_from_path(
            model_path: PathBuf,
        ) -> Result<Self, WhisperAsrLoadError> {
            if !model_path.is_file() {
                return Err(MissingFixtureModel::new(model_path).into());
            }

            let context =
                WhisperContext::new_with_params(&model_path, WhisperContextParameters::default())
                    .map_err(|error| WhisperAsrError::backend(error.to_string()))?;

            Ok(Self { context })
        }

        pub(crate) fn transcribe(
            &self,
            audio: &AudioSegment,
        ) -> Result<TranscriptDraft, WhisperAsrError> {
            let samples = prepare_audio(audio);
            let mut state = self
                .context
                .create_state()
                .map_err(|error| WhisperAsrError::backend(error.to_string()))?;

            let mut params = FullParams::new(SamplingStrategy::BeamSearch {
                beam_size: 5,
                patience: -1.0,
            });
            params.set_n_threads(1);
            params.set_language(Some("en"));
            params.set_translate(false);
            params.set_no_timestamps(true);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);

            state
                .full(params, &samples)
                .map_err(|error| WhisperAsrError::backend(error.to_string()))?;

            let segments = state
                .as_iter()
                .map(|segment| {
                    segment
                        .to_str_lossy()
                        .map(|text| text.into_owned())
                        .map_err(|error| WhisperAsrError::backend(error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok(TranscriptDraft {
                text: segments
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                metadata: TranscriptMetadata {
                    engine_name: ENGINE_NAME.to_owned(),
                    engine_version: WHISPER_RS_VERSION.to_owned(),
                    confidence: None,
                },
            })
        }
    }

    fn fixture_model_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/whisper")
            .join(PRIMARY_FIXTURE_MODEL_FILE)
    }

    fn prepare_audio(audio: &AudioSegment) -> Vec<f32> {
        debug_assert_eq!(audio.sample_rate_hz, 16_000);
        debug_assert_eq!(audio.channels, 1);
        audio.samples_f32_mono.clone()
    }
}

impl WhisperAsr {
    pub fn load_fixture_model() -> Result<Self, WhisperAsrLoadError> {
        let backend = backend::WhisperBackend::load_fixture_model()?;
        Ok(Self { backend })
    }
}

impl AsrPort for WhisperAsr {
    type Error = WhisperAsrError;

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            name: ENGINE_NAME.to_owned(),
            version: WHISPER_RS_VERSION.to_owned(),
            supports_streaming: false,
            supports_word_timestamps: false,
            supports_confidence: false,
            supports_gpu: false,
            supports_incremental_updates: false,
        }
    }

    fn transcribe(&self, audio: &AudioSegment) -> Result<TranscriptDraft, Self::Error> {
        self.backend.transcribe(audio)
    }
}

#[cfg(test)]
mod tests {
    use super::{AsrPort, WhisperAsr};
    use idiolect_test_support::fixtures::restart_traffic_fixture_16khz_mono;

    #[test]
    fn whisper_transcribes_fixture_audio() {
        let adapter = WhisperAsr::load_fixture_model().expect("fixture model should be present");
        let audio = restart_traffic_fixture_16khz_mono();
        let draft = adapter
            .transcribe(&audio)
            .expect("fixture audio should transcribe");
        let text = draft.text.to_lowercase();

        assert!(text.contains("restart"));
        assert!(text.contains("traffic"));
        assert_eq!(draft.metadata.engine_name, "whisper-rs");
        assert_eq!(draft.metadata.engine_version, "0.16.0");
    }

    #[test]
    fn whisper_reports_typed_error_for_missing_fixture_model() {
        let path = std::env::temp_dir().join(format!(
            "idiolect-missing-whisper-model-{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let result = super::backend::WhisperBackend::load_model_from_path(path);

        assert!(matches!(
            result,
            Err(super::WhisperAsrLoadError::MissingFixtureModel(_))
        ));
    }

    #[test]
    fn whisper_reports_typed_error_for_invalid_fixture_model() {
        let path = std::env::temp_dir().join(format!(
            "idiolect-invalid-whisper-model-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, b"not a whisper model").expect("invalid model fixture should write");

        let result = super::backend::WhisperBackend::load_model_from_path(path.clone());
        let _ = std::fs::remove_file(&path);

        assert!(matches!(
            result,
            Err(super::WhisperAsrLoadError::Backend(_))
        ));
    }

    #[test]
    fn whisper_reports_capabilities_without_backend_type_leakage() {
        let adapter = WhisperAsr::load_fixture_model().expect("fixture model should be present");
        let capabilities = adapter.capabilities();

        assert_eq!(capabilities.name, "whisper-rs");
        assert_eq!(capabilities.version, "0.16.0");
        assert!(!capabilities.supports_streaming);
        assert!(!capabilities.supports_word_timestamps);
        assert!(!capabilities.supports_confidence);
        assert!(!capabilities.supports_gpu);
        assert!(!capabilities.supports_incremental_updates);
        assert!(!std::any::type_name::<WhisperAsr>().contains("whisper_rs"));
    }
}
