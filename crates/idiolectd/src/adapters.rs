use std::convert::Infallible;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use idiolect_adapter_cpal::{CpalAudioInput, CpalAudioInputError};
use idiolect_adapter_translate::CommandTranslator;
use idiolect_adapter_vad::VadAdapter;
use idiolect_adapter_whisper::{WhisperAsr, WhisperDecodeTask, WhisperOptions};
use idiolect_common::config::TranslationConfig;
use idiolect_common::ids::ImeSessionId;
use idiolect_ports::asr::{AdapterCapabilities, AsrPort, TranscriptDraft, TranscriptMetadata};
use idiolect_ports::audio::{AudioInputPort, AudioSegment};
use idiolect_ports::translation::{TranslationPort, TranslationRequest};
use idiolect_ports::vad::VadPort;
use idiolect_test_support::fixtures::{
    speech_and_silence_fixture_16khz_mono, speech_pause_speech_fixture_16khz_mono,
};

/// The reserved device name that yields a deterministic in-memory fixture clip
/// instead of opening real hardware. Used by tests and CI.
pub(crate) const FIXTURE_DEVICE: &str = "fixture";

/// A reserved device name that behaves like a real microphone for the recording
/// *lifecycle* (so [`is_live_capture`] is true and the start/stop toggle path runs)
/// but yields the deterministic fixture clip on stop instead of opening hardware.
/// Lets tests drive the live capture toggle deterministically in a headless box.
pub(crate) const FIXTURE_LIVE_DEVICE: &str = "fixture-live";

/// A reserved device name that exercises the *streaming* path: live lifecycle
/// like [`FIXTURE_LIVE_DEVICE`], but the first mid-capture poll drains a canned
/// speech–pause–speech clip, so pause-triggered segmentation can be driven
/// deterministically without hardware.
pub(crate) const FIXTURE_STREAM_DEVICE: &str = "fixture-stream";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeAdapterProfile {
    pub(crate) audio_input_device: String,
    pub(crate) vad_engine: String,
    pub(crate) asr_engine: String,
    /// Resolved path to the configured Whisper model file. If it does not exist,
    /// the bundled fixture model is used as a fallback.
    pub(crate) whisper_model_path: PathBuf,
    pub(crate) asr_use_gpu: bool,
    pub(crate) asr_language: String,
    pub(crate) asr_threads: u32,
}

#[derive(Debug)]
pub(crate) struct RuntimeAdapterError {
    code: &'static str,
    message: String,
    source: Option<Box<dyn Error + 'static>>,
}

impl RuntimeAdapterError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    fn with_source<E>(code: &'static str, message: impl Into<String>, error: E) -> Self
    where
        E: Error + 'static,
    {
        Self {
            code,
            message: message.into(),
            source: Some(Box::new(error)),
        }
    }

    pub(crate) fn code(&self) -> &'static str {
        self.code
    }
}

impl Display for RuntimeAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RuntimeAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref()
    }
}

/// An in-progress recording. The fixture variant produces a deterministic clip
/// on stop; the live variant owns an open microphone stream that accumulates
/// samples between [`begin_capture`] and [`finish_capture`].
///
/// `CpalAudioInput` wraps a `cpal::Stream`, which is `!Send`, so a
/// `RuntimeCapture` must never cross threads. The run loop owns it on the single
/// connection-handling thread, which satisfies that constraint.
pub(crate) enum RuntimeCapture {
    Fixture,
    /// Live-lifecycle marker that resolves to the deterministic fixture clip on
    /// stop (see [`FIXTURE_LIVE_DEVICE`]). No hardware is opened.
    FixtureLive,
    /// Live-lifecycle marker whose first poll drains a canned
    /// speech–pause–speech clip (see [`FIXTURE_STREAM_DEVICE`]).
    FixtureStream {
        drained: bool,
    },
    Live {
        input: CpalAudioInput,
        session_id: ImeSessionId,
    },
}

/// Whether this profile records from real hardware (start/stop bounded) rather
/// than yielding the instantaneous fixture clip.
pub(crate) fn is_live_capture(profile: &RuntimeAdapterProfile) -> bool {
    profile.audio_input_device != FIXTURE_DEVICE
}

/// Opens the configured input and starts recording. For the fixture device this
/// is a no-op marker; for a real device it opens the microphone and begins
/// buffering audio immediately.
pub(crate) fn begin_capture(
    profile: &RuntimeAdapterProfile,
) -> Result<RuntimeCapture, RuntimeAdapterError> {
    if profile.audio_input_device == FIXTURE_DEVICE {
        return Ok(RuntimeCapture::Fixture);
    }
    if profile.audio_input_device == FIXTURE_LIVE_DEVICE {
        return Ok(RuntimeCapture::FixtureLive);
    }
    if profile.audio_input_device == FIXTURE_STREAM_DEVICE {
        return Ok(RuntimeCapture::FixtureStream { drained: false });
    }

    let mut input = if profile.audio_input_device == "default" {
        CpalAudioInput::open_default().map_err(map_cpal_error)?
    } else {
        CpalAudioInput::open_device_by_name(&profile.audio_input_device).map_err(map_cpal_error)?
    };
    let session_id = ImeSessionId::new();
    input.start_capture(session_id).map_err(map_cpal_error)?;
    Ok(RuntimeCapture::Live { input, session_id })
}

/// Stops the recording and returns the captured audio. For the fixture device
/// this returns the deterministic clip; for a real device it ends the
/// microphone stream and drains the buffered samples.
pub(crate) fn finish_capture(capture: RuntimeCapture) -> Result<AudioSegment, RuntimeAdapterError> {
    match capture {
        RuntimeCapture::Fixture | RuntimeCapture::FixtureLive => {
            Ok(speech_and_silence_fixture_16khz_mono())
        }
        RuntimeCapture::FixtureStream { drained } => {
            // If the streaming pump already drained the clip, only an empty tail
            // remains; without a pump (translation off) it degrades to a normal
            // stop-time capture of the whole clip.
            if drained {
                Ok(empty_processing_segment())
            } else {
                Ok(speech_pause_speech_fixture_16khz_mono())
            }
        }
        RuntimeCapture::Live {
            mut input,
            session_id,
        } => {
            // Microphones capture at their native rate (e.g. 44.1/48 kHz), but
            // the Opus codec and Whisper need 16 kHz — resample before the rest
            // of the pipeline.
            let captured = input.stop_capture(session_id).map_err(map_cpal_error)?;
            Ok(resample_to_16k_mono(captured))
        }
    }
}

/// Drains the samples accumulated since the last poll while the capture keeps
/// running. Live captures return raw audio at the device's native rate (the
/// streaming pump resamples incrementally); fixture captures return their
/// deterministic clips.
pub(crate) fn poll_capture(
    capture: &mut RuntimeCapture,
) -> Result<AudioSegment, RuntimeAdapterError> {
    match capture {
        RuntimeCapture::Fixture | RuntimeCapture::FixtureLive => Ok(empty_processing_segment()),
        RuntimeCapture::FixtureStream { drained } => {
            if *drained {
                // The speaker has gone quiet: every later poll observes one
                // second of silence, so silence-driven behaviour (the auto-stop
                // threshold) advances in audio time without wall-clock waits.
                Ok(AudioSegment {
                    sample_rate_hz: PROCESSING_RATE_HZ,
                    channels: 1,
                    duration_ms: 1_000,
                    samples_f32_mono: vec![0.0; PROCESSING_RATE_HZ as usize],
                })
            } else {
                *drained = true;
                Ok(speech_pause_speech_fixture_16khz_mono())
            }
        }
        RuntimeCapture::Live { input, session_id } => {
            input.poll_captured(*session_id).map_err(map_cpal_error)
        }
    }
}

fn empty_processing_segment() -> AudioSegment {
    AudioSegment {
        sample_rate_hz: PROCESSING_RATE_HZ,
        channels: 1,
        duration_ms: 0,
        samples_f32_mono: Vec::new(),
    }
}

/// The pipeline's working rate (Opus-supported and what Whisper expects).
const PROCESSING_RATE_HZ: u32 = 16_000;

/// Linear-interpolation resample of mono f32 audio to 16 kHz. Adequate for
/// speech; keeps the daemon dependency-free.
fn resample_to_16k_mono(segment: AudioSegment) -> AudioSegment {
    if segment.sample_rate_hz == PROCESSING_RATE_HZ || segment.samples_f32_mono.is_empty() {
        return segment;
    }
    let src_rate = f64::from(segment.sample_rate_hz);
    let ratio = f64::from(PROCESSING_RATE_HZ) / src_rate;
    let out_len = ((segment.samples_f32_mono.len() as f64) * ratio).round() as usize;
    let samples = &segment.samples_f32_mono;
    let mut out = Vec::with_capacity(out_len);
    for index in 0..out_len {
        let src_pos = index as f64 / ratio;
        let base = src_pos as usize;
        let frac = (src_pos - base as f64) as f32;
        let a = samples.get(base).copied().unwrap_or(0.0);
        let b = samples.get(base + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    AudioSegment {
        sample_rate_hz: PROCESSING_RATE_HZ,
        channels: 1,
        duration_ms: (out.len() as u64 * 1000 / u64::from(PROCESSING_RATE_HZ)) as u32,
        samples_f32_mono: out,
    }
}

/// Incremental counterpart of [`resample_to_16k_mono`] for the streaming pump:
/// resamples the mic's native rate to 16 kHz chunk by chunk, carrying the
/// interpolation position across chunk boundaries so the output is identical to
/// resampling the whole take at once (minus at most one held-back tail sample).
pub(crate) struct StreamingResampler {
    src_rate_hz: u32,
    /// Source samples per output sample (e.g. 3.0 for 48 kHz → 16 kHz).
    step: f64,
    /// Fractional read position of the next output sample within `pending`.
    next_src_pos: f64,
    pending: Vec<f32>,
}

impl StreamingResampler {
    pub(crate) fn new(src_rate_hz: u32) -> Self {
        Self {
            src_rate_hz,
            step: f64::from(src_rate_hz) / f64::from(PROCESSING_RATE_HZ),
            next_src_pos: 0.0,
            pending: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        if self.src_rate_hz == PROCESSING_RATE_HZ {
            return samples.to_vec();
        }
        self.pending.extend_from_slice(samples);

        let mut out = Vec::new();
        // Interpolation needs the sample after the read position, so the last
        // pending sample is held back until the next chunk arrives.
        loop {
            let base = self.next_src_pos as usize;
            if base + 1 >= self.pending.len() {
                break;
            }
            let frac = (self.next_src_pos - base as f64) as f32;
            let a = self.pending[base];
            let b = self.pending[base + 1];
            out.push(a + (b - a) * frac);
            self.next_src_pos += self.step;
        }

        // Drop fully-consumed source samples, keeping the one the next output
        // still interpolates from.
        let consumed = (self.next_src_pos as usize).min(self.pending.len().saturating_sub(1));
        self.pending.drain(..consumed);
        self.next_src_pos -= consumed as f64;
        out
    }
}

fn map_cpal_error(error: CpalAudioInputError) -> RuntimeAdapterError {
    RuntimeAdapterError::with_source(
        "audio-unavailable",
        format!("microphone capture failed: {error}"),
        error,
    )
}

/// Transcribes — and, when translation is enabled, translates — the captured
/// audio. Routing:
/// - translation disabled → plain transcription in the configured ASR language;
/// - target `"en"` with no external command → Whisper's built-in translate task
///   (any input language → English inside the engine, no extra tooling);
/// - an external `translation.command` → transcribe in the input language, then
///   pipe the text through the command for any-pair translation;
/// - any other target with no command → a typed `translation-unavailable` error
///   (never a silent fallback to untranslated text).
pub(crate) fn transcribe_translated(
    profile: &RuntimeAdapterProfile,
    translation: &TranslationConfig,
    audio: &AudioSegment,
) -> Result<TranscriptDraft, RuntimeAdapterError> {
    let speech = speech_audio(profile, audio)?;

    if !translation.enabled {
        return match profile.asr_engine.as_str() {
            "fixture" => transcribe_with_fixture(&speech),
            "whisper-rs" => {
                transcribe_with_whisper(profile, &speech, &WhisperDecodeTask::default())
            }
            other => Err(unsupported_asr_engine(other)),
        };
    }

    let translator = CommandTranslator::from_config(&translation.command);

    let draft = match profile.asr_engine.as_str() {
        // The deterministic fixture engine plays the role of "transcript in the
        // input language" for tests; the engine-internal translate task is a
        // no-op for it (its output is already English text).
        "fixture" => transcribe_with_fixture(&speech)?,
        "whisper-rs" => {
            let engine_translates = translator.is_none() && translation.output_language == "en";
            transcribe_with_whisper(
                profile,
                &speech,
                &WhisperDecodeTask {
                    language: Some(translation.input_language.clone()),
                    translate_to_english: engine_translates,
                },
            )?
        }
        other => return Err(unsupported_asr_engine(other)),
    };

    let Some(translator) = translator else {
        return if translation.output_language == "en" {
            Ok(draft)
        } else {
            Err(RuntimeAdapterError::new(
                "translation-unavailable",
                format!(
                    "translating to '{}' needs translation.command (only English works without one)",
                    translation.output_language
                ),
            ))
        };
    };

    let translated = translator
        .translate(&TranslationRequest {
            text: &draft.text,
            source_language: &translation.input_language,
            target_language: &translation.output_language,
        })
        .map_err(|error| {
            RuntimeAdapterError::with_source(
                "translation-unavailable",
                format!("translation command failed: {error}"),
                error,
            )
        })?;

    Ok(TranscriptDraft {
        text: translated,
        metadata: draft.metadata,
    })
}

fn unsupported_asr_engine(engine: &str) -> RuntimeAdapterError {
    RuntimeAdapterError::new(
        "asr-unavailable",
        format!("ASR engine '{engine}' is not supported by idiolectd run"),
    )
}

pub(crate) use idiolect_process::notify_user;

/// Extract the spoken audio for transcription. Recording runs from the user's
/// Super+T (start) to Super+T (stop); VAD is used only to drop leading/trailing
/// silence and noise. All speech segments are concatenated, so a pause in the
/// middle of dictation does NOT truncate the utterance — we keep everything the
/// user said until they stop.
///
/// Only WebRTC VAD is implemented; `silero` (the config default) is accepted and
/// served by the WebRTC adapter so out-of-the-box configs work.
fn speech_audio(
    profile: &RuntimeAdapterProfile,
    audio: &AudioSegment,
) -> Result<AudioSegment, RuntimeAdapterError> {
    if !matches!(profile.vad_engine.as_str(), "webrtc" | "silero") {
        return Err(RuntimeAdapterError::new(
            "vad-unavailable",
            format!(
                "VAD engine '{}' is not supported by idiolectd run",
                profile.vad_engine
            ),
        ));
    }

    let mut vad = VadAdapter::new();
    let segments = vad.segment(audio).map_err(|error| {
        RuntimeAdapterError::with_source("vad-unavailable", format!("VAD failed: {error}"), error)
    })?;

    let mut iter = segments.into_iter();
    let first = iter.next().ok_or_else(|| {
        RuntimeAdapterError::new("vad-unavailable", "VAD did not find a speech segment")
    })?;
    let sample_rate = first.sample_rate_hz;
    let channels = first.channels;
    let mut samples = first.samples_f32_mono;
    for segment in iter {
        samples.extend_from_slice(&segment.samples_f32_mono);
    }
    let duration_ms = if sample_rate > 0 {
        (samples.len() as u64 * 1000 / u64::from(sample_rate)) as u32
    } else {
        first.duration_ms
    };
    Ok(AudioSegment {
        sample_rate_hz: sample_rate,
        channels,
        duration_ms,
        samples_f32_mono: samples,
    })
}

fn transcribe_with_fixture(audio: &AudioSegment) -> Result<TranscriptDraft, RuntimeAdapterError> {
    match FixtureAsr.transcribe(audio) {
        Ok(draft) => Ok(draft),
        Err(error) => match error {},
    }
}

thread_local! {
    // Loading the model and (for GPU builds) initialising the CUDA context is
    // expensive, so the Whisper engine is built once on the run-loop thread and
    // reused for every utterance rather than reloaded per transcription.
    static WHISPER: std::cell::RefCell<Option<WhisperAsr>> = const { std::cell::RefCell::new(None) };
}

/// Loads the Whisper engine for this profile: the configured model when its file
/// exists, otherwise the bundled fixture model (so a misconfigured or
/// undownloaded model degrades to a working — if small — engine instead of
/// failing dictation outright).
fn load_whisper_engine(profile: &RuntimeAdapterProfile) -> Result<WhisperAsr, RuntimeAdapterError> {
    if profile.whisper_model_path.is_file() {
        let options = WhisperOptions {
            use_gpu: profile.asr_use_gpu,
            gpu_device: 0,
            language: profile.asr_language.clone(),
            n_threads: profile.asr_threads,
            // The desktop keeps beam search (the default): it has the GPU/CPU headroom
            // for the small accuracy gain. The mobile facade opts into greedy instead.
            beam_size: WhisperOptions::default().beam_size,
        };
        eprintln!(
            "whisper: loading model {} (gpu={})",
            profile.whisper_model_path.display(),
            profile.asr_use_gpu
        );
        return WhisperAsr::load(profile.whisper_model_path.clone(), options).map_err(|error| {
            RuntimeAdapterError::with_source(
                "asr-unavailable",
                format!(
                    "whisper-rs model {} failed to load: {error}",
                    profile.whisper_model_path.display()
                ),
                error,
            )
        });
    }

    eprintln!(
        "whisper: configured model {} not found; falling back to bundled fixture model",
        profile.whisper_model_path.display()
    );
    WhisperAsr::load_fixture_model().map_err(|error| {
        RuntimeAdapterError::with_source(
            "asr-unavailable",
            format!("whisper-rs fixture model unavailable: {error}"),
            error,
        )
    })
}

fn transcribe_with_whisper(
    profile: &RuntimeAdapterProfile,
    audio: &AudioSegment,
    task: &WhisperDecodeTask,
) -> Result<TranscriptDraft, RuntimeAdapterError> {
    WHISPER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(load_whisper_engine(profile)?);
        }

        slot.as_ref()
            .expect("whisper engine was just initialised")
            .transcribe_with_task(audio, task)
            .map_err(|error| {
                RuntimeAdapterError::with_source(
                    "asr-unavailable",
                    format!("whisper-rs transcription failed: {error}"),
                    error,
                )
            })
    })
}

struct FixtureAsr;

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
            text: "restart traffic".to_owned(),
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
    use idiolect_common::config::TranslationConfig;

    use super::{
        begin_capture, finish_capture, is_live_capture, transcribe_translated,
        RuntimeAdapterProfile, RuntimeCapture,
    };

    fn fixture_profile() -> RuntimeAdapterProfile {
        RuntimeAdapterProfile {
            audio_input_device: "fixture".to_owned(),
            vad_engine: "webrtc".to_owned(),
            asr_engine: "whisper-rs".to_owned(),
            whisper_model_path: std::path::PathBuf::from("/nonexistent/model.bin"),
            asr_use_gpu: false,
            asr_language: "en".to_owned(),
            asr_threads: 1,
        }
    }

    fn fixture_asr_profile() -> RuntimeAdapterProfile {
        RuntimeAdapterProfile {
            asr_engine: "fixture".to_owned(),
            ..fixture_profile()
        }
    }

    fn fixture_audio() -> idiolect_ports::audio::AudioSegment {
        finish_capture(RuntimeCapture::Fixture).expect("fixture capture")
    }

    /// Writes an executable translator stub honouring the subprocess contract.
    fn translator_script(tag: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!(
            "idiolectd-translate-test-{tag}-{}",
            std::process::id()
        ));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod script");
        path
    }

    fn translation(input: &str, output: &str, command: &str) -> TranslationConfig {
        TranslationConfig {
            enabled: true,
            input_language: input.to_owned(),
            output_language: output.to_owned(),
            command: command.to_owned(),
        }
    }

    #[test]
    fn disabled_translation_is_plain_transcription() {
        let settings = TranslationConfig::default();
        let draft = transcribe_translated(&fixture_asr_profile(), &settings, &fixture_audio())
            .expect("disabled translation should transcribe");
        assert_eq!(draft.text, "restart traffic");
    }

    #[test]
    fn configured_command_translates_the_transcript() {
        // Any-pair translation: the transcript is piped through the external
        // translator with the configured language pair.
        let script = translator_script(
            "pair",
            r#"printf '[%s>%s] ' "$1" "$2"; tr '[:lower:]' '[:upper:]'"#,
        );
        let settings = translation("sv", "ja", script.to_str().expect("utf8 path"));

        let draft = transcribe_translated(&fixture_asr_profile(), &settings, &fixture_audio())
            .expect("command translation should succeed");

        assert_eq!(draft.text, "[sv>ja] RESTART TRAFFIC");
        let _ = std::fs::remove_file(&script);
    }

    #[test]
    fn english_target_without_command_uses_the_engine_task() {
        // No external tooling needed for X→English: Whisper's built-in translate
        // task handles it inside the engine, so this must succeed with an empty
        // command. (On the en-only fixture model the task is a no-op decode; a
        // multilingual model is what makes it a real translation.)
        let settings = translation("auto", "en", "");
        let draft = transcribe_translated(&fixture_profile(), &settings, &fixture_audio())
            .expect("en target must work without a command");
        let text = draft.text.to_lowercase();
        assert!(text.contains("restart") && text.contains("traffic"));
    }

    #[test]
    fn non_english_target_without_command_is_a_typed_error() {
        let settings = translation("auto", "ja", "");
        let error = transcribe_translated(&fixture_profile(), &settings, &fixture_audio())
            .expect_err("ja target without a command must fail");
        assert_eq!(error.code(), "translation-unavailable");
    }

    #[test]
    fn failing_translator_command_is_a_typed_error_not_raw_text() {
        // A broken translator must never silently fall back to committing the
        // untranslated transcript — the user asked for language B, not A.
        let script = translator_script("broken", "exit 9");
        let settings = translation("sv", "ja", script.to_str().expect("utf8 path"));

        let error = transcribe_translated(&fixture_asr_profile(), &settings, &fixture_audio())
            .expect_err("broken translator must surface an error");

        assert_eq!(error.code(), "translation-unavailable");
        let _ = std::fs::remove_file(&script);
    }

    #[test]
    fn real_vad_and_whisper_rs_fixture_profile_transcribes_fixture_audio() {
        let profile = fixture_profile();

        let capture = begin_capture(&profile).expect("fixture capture should begin");
        let audio = finish_capture(capture).expect("fixture audio should capture");
        let draft = transcribe_translated(&profile, &TranslationConfig::default(), &audio)
            .expect("fixture should transcribe");
        let text = draft.text.to_lowercase();

        assert!(text.contains("restart"));
        assert!(text.contains("traffic"));
        assert_eq!(draft.metadata.engine_name, "whisper-rs");
    }

    #[test]
    fn fixture_device_is_not_live_capture() {
        assert!(!is_live_capture(&fixture_profile()));
    }

    #[test]
    fn real_device_names_are_live_capture() {
        for device in ["default", "hw:0,0", "USB Microphone"] {
            let profile = RuntimeAdapterProfile {
                audio_input_device: device.to_owned(),
                ..fixture_profile()
            };
            assert!(
                is_live_capture(&profile),
                "device {device} should be treated as live"
            );
        }
    }

    #[test]
    fn fixture_capture_yields_non_empty_audio() {
        let audio = finish_capture(RuntimeCapture::Fixture).expect("fixture capture");
        assert!(audio.sample_count() > 0);
        assert_eq!(audio.channels, 1);
    }

    mod streaming_resampler {
        use super::super::{resample_to_16k_mono, StreamingResampler};
        use idiolect_ports::audio::AudioSegment;

        fn sine_48k(samples: usize) -> Vec<f32> {
            (0..samples)
                .map(|index| (index as f32 * 0.013).sin())
                .collect()
        }

        // Feeding the capture in arbitrary chunks must produce (almost) the same
        // signal as the one-shot resampler — no boundary artifacts, no drift.
        #[test]
        fn chunked_output_matches_one_shot_resampling() {
            let samples = sine_48k(48_000);
            let reference = resample_to_16k_mono(AudioSegment {
                sample_rate_hz: 48_000,
                channels: 1,
                duration_ms: 1_000,
                samples_f32_mono: samples.clone(),
            });

            let mut resampler = StreamingResampler::new(48_000);
            let mut streamed = Vec::new();
            // Deliberately ragged chunk sizes (not multiples of the 3:1 ratio).
            for chunk in samples.chunks(1_001) {
                streamed.extend(resampler.push(chunk));
            }

            // The streaming variant may hold back up to one source sample at the
            // tail; everything it produced must match the reference closely.
            assert!(reference.samples_f32_mono.len() - streamed.len() <= 2);
            for (index, (streamed_sample, reference_sample)) in
                streamed.iter().zip(&reference.samples_f32_mono).enumerate()
            {
                assert!(
                    (streamed_sample - reference_sample).abs() < 1e-4,
                    "sample {index} diverged: {streamed_sample} vs {reference_sample}"
                );
            }
        }

        #[test]
        fn native_rate_passes_through_unchanged() {
            let mut resampler = StreamingResampler::new(16_000);
            let chunk = vec![0.5_f32; 160];
            assert_eq!(resampler.push(&chunk), chunk);
        }
    }
}
