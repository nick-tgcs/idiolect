#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureQuality {
    Low,
    High,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrainingCandidateSource {
    ImePreeditCorrection,
    AcceptedWithoutEdit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrainingCandidate {
    source: TrainingCandidateSource,
    capture_quality: CaptureQuality,
    trust_score: f32,
    raw_text: String,
    target_text: String,
}

impl TrainingCandidate {
    #[must_use]
    pub fn from_preedit_correction(raw_text: &str, corrected_text: &str) -> Option<Self> {
        if raw_text == corrected_text {
            return None;
        }

        Some(Self {
            source: TrainingCandidateSource::ImePreeditCorrection,
            capture_quality: CaptureQuality::High,
            trust_score: 1.0,
            raw_text: raw_text.to_owned(),
            target_text: corrected_text.to_owned(),
        })
    }

    #[must_use]
    pub fn from_acceptance(committed_text: &str) -> Self {
        Self {
            source: TrainingCandidateSource::AcceptedWithoutEdit,
            capture_quality: CaptureQuality::Low,
            trust_score: 0.6,
            raw_text: committed_text.to_owned(),
            target_text: committed_text.to_owned(),
        }
    }

    #[must_use]
    pub fn source(&self) -> TrainingCandidateSource {
        self.source
    }

    #[must_use]
    pub fn capture_quality(&self) -> CaptureQuality {
        self.capture_quality
    }

    #[must_use]
    pub fn trust_score(&self) -> f32 {
        self.trust_score
    }

    #[must_use]
    pub fn raw_text(&self) -> &str {
        &self.raw_text
    }

    #[must_use]
    pub fn target_text(&self) -> &str {
        &self.target_text
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureQuality, TrainingCandidate, TrainingCandidateSource};

    #[test]
    fn preedit_correction_creates_high_quality_candidate() {
        let candidate =
            TrainingCandidate::from_preedit_correction("restart traffic", "restart Traefik")
                .expect("changed preedit should create candidate");

        assert_eq!(
            candidate.source(),
            TrainingCandidateSource::ImePreeditCorrection
        );
        assert_eq!(candidate.capture_quality(), CaptureQuality::High);
        assert_eq!(candidate.trust_score(), 1.0);
    }

    #[test]
    fn accepted_without_edit_creates_weak_candidate() {
        let candidate = TrainingCandidate::from_acceptance("deploy the container");

        assert_eq!(
            candidate.source(),
            TrainingCandidateSource::AcceptedWithoutEdit
        );
        assert_eq!(candidate.capture_quality(), CaptureQuality::Low);
        assert_eq!(candidate.trust_score(), 0.6);
    }

    #[test]
    fn candidate_keeps_training_text_context() {
        let correction =
            TrainingCandidate::from_preedit_correction("restart traffic", "restart Traefik")
                .expect("changed preedit should create candidate");
        assert_eq!(correction.raw_text(), "restart traffic");
        assert_eq!(correction.target_text(), "restart Traefik");

        let accepted = TrainingCandidate::from_acceptance("deploy the container");
        assert_eq!(accepted.raw_text(), "deploy the container");
        assert_eq!(accepted.target_text(), "deploy the container");
    }
}
