use idiolect_common::ids::ImeSessionId;

#[derive(Clone, Debug, PartialEq)]
pub struct AudioSegment {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub duration_ms: u32,
    pub samples_f32_mono: Vec<f32>,
}

impl AudioSegment {
    pub fn sample_count(&self) -> usize {
        self.samples_f32_mono.len()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EncodedAudio {
    pub codec_name: String,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptMetadata {
    pub engine_name: String,
    pub engine_version: String,
    pub confidence: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptDraft {
    pub text: String,
    pub metadata: TranscriptMetadata,
}

pub trait AudioInputPort {
    type Error;

    fn start_capture(&mut self, session_id: ImeSessionId) -> Result<(), Self::Error>;
    fn stop_capture(&mut self, session_id: ImeSessionId) -> Result<AudioSegment, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::{AudioSegment, EncodedAudio, TranscriptDraft, TranscriptMetadata};

    #[test]
    fn audio_segment_has_required_fields_and_sample_count() {
        let segment = AudioSegment {
            sample_rate_hz: 16_000,
            channels: 1,
            duration_ms: 1_000,
            samples_f32_mono: vec![0.0; 16_000],
        };

        assert_eq!(segment.sample_rate_hz, 16_000);
        assert_eq!(segment.channels, 1);
        assert_eq!(segment.duration_ms, 1_000);
        assert_eq!(segment.samples_f32_mono.len(), 16_000);
        assert_eq!(segment.sample_count(), 16_000);
    }

    #[test]
    fn encoded_audio_has_required_fields() {
        let encoded = EncodedAudio {
            codec_name: "fixture-codec".to_owned(),
            sample_rate_hz: 16_000,
            channels: 1,
            payload: vec![1, 2, 3],
        };

        assert_eq!(encoded.codec_name, "fixture-codec");
        assert_eq!(encoded.sample_rate_hz, 16_000);
        assert_eq!(encoded.channels, 1);
        assert_eq!(encoded.payload, [1, 2, 3]);
    }

    #[test]
    fn transcript_draft_has_required_metadata_fields() {
        let draft = TranscriptDraft {
            text: "restart traffic".to_owned(),
            metadata: TranscriptMetadata {
                engine_name: "fixture-asr".to_owned(),
                engine_version: "0.1.0".to_owned(),
                confidence: Some(1.0),
            },
        };

        assert_eq!(draft.text, "restart traffic");
        assert_eq!(draft.metadata.engine_name, "fixture-asr");
        assert_eq!(draft.metadata.engine_version, "0.1.0");
        assert_eq!(draft.metadata.confidence, Some(1.0));
    }
}
