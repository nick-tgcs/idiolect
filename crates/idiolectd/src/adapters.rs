use std::convert::Infallible;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use idiolect_adapter_cpal::{CpalAudioInput, CpalAudioInputError};
use idiolect_adapter_vad::VadAdapter;
use idiolect_adapter_whisper::{WhisperAsr, WhisperOptions};
use idiolect_common::ids::ImeSessionId;
use idiolect_ports::asr::{AdapterCapabilities, AsrPort, TranscriptDraft, TranscriptMetadata};
use idiolect_ports::audio::{AudioInputPort, AudioSegment};
use idiolect_ports::vad::VadPort;
use idiolect_test_support::fixtures::speech_and_silence_fixture_16khz_mono;

/// The reserved device name that yields a deterministic in-memory fixture clip
/// instead of opening real hardware. Used by tests and CI.
pub(crate) const FIXTURE_DEVICE: &str = "fixture";

/// A reserved device name that behaves like a real microphone for the recording
/// *lifecycle* (so [`is_live_capture`] is true and the start/stop toggle path runs)
/// but yields the deterministic fixture clip on stop instead of opening hardware.
/// Lets tests drive the live capture toggle deterministically in a headless box.
pub(crate) const FIXTURE_LIVE_DEVICE: &str = "fixture-live";

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

fn map_cpal_error(error: CpalAudioInputError) -> RuntimeAdapterError {
    RuntimeAdapterError::with_source(
        "audio-unavailable",
        format!("microphone capture failed: {error}"),
        error,
    )
}

pub(crate) fn transcribe_audio(
    profile: &RuntimeAdapterProfile,
    audio: &AudioSegment,
) -> Result<TranscriptDraft, RuntimeAdapterError> {
    let speech = speech_audio(profile, audio)?;

    match profile.asr_engine.as_str() {
        "fixture" => transcribe_with_fixture(&speech),
        "whisper-rs" => transcribe_with_whisper(profile, &speech),
        other => Err(RuntimeAdapterError::new(
            "asr-unavailable",
            format!("ASR engine '{other}' is not supported by idiolectd run"),
        )),
    }
}

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
) -> Result<TranscriptDraft, RuntimeAdapterError> {
    WHISPER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(load_whisper_engine(profile)?);
        }

        slot.as_ref()
            .expect("whisper engine was just initialised")
            .transcribe(audio)
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
    use super::{
        begin_capture, finish_capture, is_live_capture, transcribe_audio, RuntimeAdapterProfile,
        RuntimeCapture,
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

    #[test]
    fn real_vad_and_whisper_rs_fixture_profile_transcribes_fixture_audio() {
        let profile = fixture_profile();

        let capture = begin_capture(&profile).expect("fixture capture should begin");
        let audio = finish_capture(capture).expect("fixture audio should capture");
        let draft = transcribe_audio(&profile, &audio).expect("fixture should transcribe");
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
}
