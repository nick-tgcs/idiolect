use std::convert::Infallible;
use std::error::Error;
use std::fmt::{Display, Formatter};

use idiolect_adapter_vad::VadAdapter;
use idiolect_adapter_whisper::WhisperAsr;
use idiolect_ports::asr::{AdapterCapabilities, AsrPort, TranscriptDraft, TranscriptMetadata};
use idiolect_ports::audio::AudioSegment;
use idiolect_ports::vad::VadPort;
use idiolect_test_support::fixtures::speech_and_silence_fixture_16khz_mono;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeAdapterProfile {
    pub(crate) audio_input_device: String,
    pub(crate) vad_engine: String,
    pub(crate) asr_engine: String,
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

pub(crate) fn capture_audio(
    profile: &RuntimeAdapterProfile,
) -> Result<AudioSegment, RuntimeAdapterError> {
    if profile.audio_input_device == "fixture" {
        return Ok(speech_and_silence_fixture_16khz_mono());
    }

    Err(RuntimeAdapterError::new(
        "audio-unavailable",
        format!(
            "audio input profile '{}' is not available in this daemon run profile",
            profile.audio_input_device
        ),
    ))
}

pub(crate) fn transcribe_audio(
    profile: &RuntimeAdapterProfile,
    audio: &AudioSegment,
) -> Result<TranscriptDraft, RuntimeAdapterError> {
    let speech = first_speech_segment(profile, audio)?;

    match profile.asr_engine.as_str() {
        "fixture" => transcribe_with_fixture(&speech),
        "whisper-rs" => transcribe_with_whisper(&speech),
        other => Err(RuntimeAdapterError::new(
            "asr-unavailable",
            format!("ASR engine '{other}' is not supported by idiolectd run"),
        )),
    }
}

fn first_speech_segment(
    profile: &RuntimeAdapterProfile,
    audio: &AudioSegment,
) -> Result<AudioSegment, RuntimeAdapterError> {
    if profile.vad_engine != "webrtc" {
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

    segments.into_iter().next().ok_or_else(|| {
        RuntimeAdapterError::new("vad-unavailable", "VAD did not find a speech segment")
    })
}

fn transcribe_with_fixture(audio: &AudioSegment) -> Result<TranscriptDraft, RuntimeAdapterError> {
    match FixtureAsr.transcribe(audio) {
        Ok(draft) => Ok(draft),
        Err(error) => match error {},
    }
}

fn transcribe_with_whisper(audio: &AudioSegment) -> Result<TranscriptDraft, RuntimeAdapterError> {
    let whisper = WhisperAsr::load_fixture_model().map_err(|error| {
        RuntimeAdapterError::with_source(
            "asr-unavailable",
            format!("whisper-rs fixture model unavailable: {error}"),
            error,
        )
    })?;

    whisper.transcribe(audio).map_err(|error| {
        RuntimeAdapterError::with_source(
            "asr-unavailable",
            format!("whisper-rs transcription failed: {error}"),
            error,
        )
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
    use super::{capture_audio, transcribe_audio, RuntimeAdapterProfile};

    #[test]
    fn real_vad_and_whisper_rs_fixture_profile_transcribes_fixture_audio() {
        let profile = RuntimeAdapterProfile {
            audio_input_device: "fixture".to_owned(),
            vad_engine: "webrtc".to_owned(),
            asr_engine: "whisper-rs".to_owned(),
        };

        let audio = capture_audio(&profile).expect("fixture audio should capture");
        let draft = transcribe_audio(&profile, &audio).expect("fixture should transcribe");
        let text = draft.text.to_lowercase();

        assert!(text.contains("restart"));
        assert!(text.contains("traffic"));
        assert_eq!(draft.metadata.engine_name, "whisper-rs");
    }
}
