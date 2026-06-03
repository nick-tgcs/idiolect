#[derive(Clone, Debug, PartialEq)]
pub struct AudioSegment {
    sample_rate_hz: u32,
    samples_f32_mono: Vec<f32>,
}

impl AudioSegment {
    #[must_use]
    pub fn from_mono_samples(sample_rate_hz: u32, samples_f32_mono: Vec<f32>) -> Self {
        Self {
            sample_rate_hz,
            samples_f32_mono,
        }
    }

    #[must_use]
    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    #[must_use]
    pub fn samples_f32_mono(&self) -> &[f32] {
        &self.samples_f32_mono
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedAudio {
    codec: String,
    bytes: Vec<u8>,
}

impl EncodedAudio {
    #[must_use]
    pub fn new(codec: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            codec: codec.into(),
            bytes,
        }
    }

    #[must_use]
    pub fn codec(&self) -> &str {
        &self.codec
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptDraft {
    text: String,
}

impl TranscriptDraft {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrainingManifest {
    digest: String,
}

impl TrainingManifest {
    #[must_use]
    pub fn new(digest: impl Into<String>) -> Self {
        Self {
            digest: digest.into(),
        }
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrainingArtifact {
    digest: String,
}

impl TrainingArtifact {
    #[must_use]
    pub fn new(digest: impl Into<String>) -> Self {
        Self {
            digest: digest.into(),
        }
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluationReport {
    digest: String,
}

impl EvaluationReport {
    #[must_use]
    pub fn new(digest: impl Into<String>) -> Self {
        Self {
            digest: digest.into(),
        }
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioSegment, EncodedAudio, EvaluationReport, TrainingArtifact, TrainingManifest,
        TranscriptDraft,
    };

    #[test]
    fn port_contract_types_keep_owned_values() {
        let audio = AudioSegment::from_mono_samples(16_000, vec![0.0, 0.5]);
        assert_eq!(audio.sample_rate_hz(), 16_000);
        assert_eq!(audio.samples_f32_mono(), &[0.0, 0.5]);

        let encoded = EncodedAudio::new("fixture/raw", vec![1, 2, 3]);
        assert_eq!(encoded.codec(), "fixture/raw");
        assert_eq!(encoded.bytes(), &[1, 2, 3]);

        assert_eq!(
            TranscriptDraft::new("restart Traefik").text(),
            "restart Traefik"
        );
        assert_eq!(
            TrainingManifest::new("manifest-digest").digest(),
            "manifest-digest"
        );
        assert_eq!(
            TrainingArtifact::new("artifact-digest").digest(),
            "artifact-digest"
        );
        assert_eq!(
            EvaluationReport::new("report-digest").digest(),
            "report-digest"
        );
    }
}
