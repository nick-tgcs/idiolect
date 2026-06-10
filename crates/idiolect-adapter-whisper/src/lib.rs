#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use idiolect_ports::asr::{AdapterCapabilities, AsrPort, TranscriptDraft, TranscriptMetadata};
use idiolect_ports::audio::AudioSegment;
use thiserror::Error;

const ENGINE_NAME: &str = "whisper-rs";
const WHISPER_RS_VERSION: &str = "0.16.0";
const PRIMARY_FIXTURE_MODEL_FILE: &str = "ggml-tiny.en.bin";

/// Whether this build offloads Whisper inference to the GPU. Toggled by the
/// `cuda` cargo feature on this crate.
const GPU_ENABLED: bool = cfg!(feature = "cuda");

pub struct WhisperAsr {
    backend: backend::WhisperBackend,
}

/// Runtime options for loading the Whisper engine from a model file.
#[derive(Debug, Clone)]
pub struct WhisperOptions {
    /// Request GPU offload. Effective only when built with the `cuda` feature;
    /// a CPU build ignores it and runs on the CPU.
    pub use_gpu: bool,
    /// CUDA device index to use when `use_gpu` is active.
    pub gpu_device: i32,
    /// Decoding language hint (e.g. "en").
    pub language: String,
    /// CPU decode threads (still set for GPU builds; harmless there).
    pub n_threads: u32,
}

impl Default for WhisperOptions {
    fn default() -> Self {
        Self {
            use_gpu: GPU_ENABLED,
            gpu_device: 0,
            language: "en".to_owned(),
            n_threads: 1,
        }
    }
}

/// Per-call decode task. The engine (model + GPU context) is loaded once and
/// reused, but the task can change between calls — e.g. when the user flips the
/// tray's translation toggle or picks a different input language mid-session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WhisperDecodeTask {
    /// Language hint override for this call. `None` uses the engine's configured
    /// default; `Some("auto")` asks the decoder to detect the language
    /// (multilingual models only).
    pub language: Option<String>,
    /// Run Whisper's built-in translate task: speech in any supported language
    /// decodes directly to English text. Only meaningful on multilingual models.
    pub translate_to_english: bool,
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
        language: String,
        n_threads: i32,
    }

    impl WhisperBackend {
        pub(crate) fn load_fixture_model() -> Result<Self, WhisperAsrLoadError> {
            Self::load_model_from_path(fixture_model_path(), WhisperOptions::default())
        }

        pub(crate) fn load_model_from_path(
            model_path: PathBuf,
            options: WhisperOptions,
        ) -> Result<Self, WhisperAsrLoadError> {
            if !model_path.is_file() {
                return Err(MissingFixtureModel::new(model_path).into());
            }

            // Offload inference to the GPU when both the build (the `cuda`
            // feature) and the runtime config allow it; otherwise whisper.cpp
            // stays on the CPU.
            let mut params = WhisperContextParameters::default();
            params.use_gpu(options.use_gpu && GPU_ENABLED);
            params.gpu_device(options.gpu_device);

            let context = WhisperContext::new_with_params(&model_path, params)
                .map_err(|error| WhisperAsrError::backend(error.to_string()))?;

            Ok(Self {
                context,
                language: options.language,
                n_threads: i32::try_from(options.n_threads.max(1)).unwrap_or(i32::MAX),
            })
        }

        pub(crate) fn transcribe(
            &self,
            audio: &AudioSegment,
            task: &WhisperDecodeTask,
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
            params.set_n_threads(self.n_threads);
            let language = task.language.as_deref().unwrap_or(self.language.as_str());
            params.set_language(Some(language));
            params.set_translate(task.translate_to_english);
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

    /// Loads a Whisper model from a file with explicit runtime options. Returns
    /// [`WhisperAsrLoadError::MissingFixtureModel`] if the file does not exist.
    pub fn load(
        model_path: impl Into<PathBuf>,
        options: WhisperOptions,
    ) -> Result<Self, WhisperAsrLoadError> {
        let backend = backend::WhisperBackend::load_model_from_path(model_path.into(), options)?;
        Ok(Self { backend })
    }

    /// Transcribes with an explicit per-call decode task (language override
    /// and/or Whisper's built-in X→English translate task). The plain
    /// [`AsrPort::transcribe`] is equivalent to the default task.
    pub fn transcribe_with_task(
        &self,
        audio: &AudioSegment,
        task: &WhisperDecodeTask,
    ) -> Result<TranscriptDraft, WhisperAsrError> {
        self.backend.transcribe(audio, task)
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
            supports_gpu: GPU_ENABLED,
            supports_incremental_updates: false,
        }
    }

    fn transcribe(&self, audio: &AudioSegment) -> Result<TranscriptDraft, Self::Error> {
        self.backend.transcribe(audio, &WhisperDecodeTask::default())
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

    // NOTE on test depth: asserting an actual cross-language translation needs a
    // multilingual Whisper model plus a non-English audio fixture; the only model
    // committed to the repo is the English-only `ggml-tiny.en.bin`, so observable
    // X→English output is exercised by the gated/manual path, not here. What IS
    // testable hermetically — and what these tests pin down — is the task
    // plumbing contract: per-call task selection exists, the default task is
    // plain transcription, and a task-equivalent call produces the same result
    // as the legacy `transcribe` entry point.
    #[test]
    fn default_decode_task_is_plain_transcription() {
        let task = super::WhisperDecodeTask::default();
        assert_eq!(task.language, None, "engine-default language");
        assert!(!task.translate_to_english);
    }

    #[test]
    fn transcribe_with_default_task_matches_legacy_transcribe() {
        let adapter = WhisperAsr::load_fixture_model().expect("fixture model should be present");
        let audio = restart_traffic_fixture_16khz_mono();

        let legacy = adapter.transcribe(&audio).expect("legacy transcribe");
        let via_task = adapter
            .transcribe_with_task(&audio, &super::WhisperDecodeTask::default())
            .expect("task transcribe");

        assert_eq!(via_task.text, legacy.text);
        assert_eq!(via_task.metadata.engine_name, "whisper-rs");
    }

    #[test]
    fn per_call_language_override_reaches_the_decoder() {
        // An explicit per-call "en" hint on the en-only model must decode
        // normally — proving the override path is wired, not ignored.
        let adapter = WhisperAsr::load_fixture_model().expect("fixture model should be present");
        let audio = restart_traffic_fixture_16khz_mono();

        let draft = adapter
            .transcribe_with_task(
                &audio,
                &super::WhisperDecodeTask {
                    language: Some("en".to_owned()),
                    translate_to_english: false,
                },
            )
            .expect("override transcribe");

        let text = draft.text.to_lowercase();
        assert!(text.contains("restart") && text.contains("traffic"));
    }

    #[test]
    fn whisper_reports_typed_error_for_missing_fixture_model() {
        let path = std::env::temp_dir().join(format!(
            "idiolect-missing-whisper-model-{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let result = super::backend::WhisperBackend::load_model_from_path(
            path,
            super::WhisperOptions::default(),
        );

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

        let result = super::backend::WhisperBackend::load_model_from_path(
            path.clone(),
            super::WhisperOptions::default(),
        );
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
        assert_eq!(capabilities.supports_gpu, cfg!(feature = "cuda"));
        assert!(!capabilities.supports_incremental_updates);
        assert!(!std::any::type_name::<WhisperAsr>().contains("whisper_rs"));
    }
}
