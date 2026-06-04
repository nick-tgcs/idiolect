pub enum CandidateEvidence {
    PreeditCorrection {
        raw_text: String,
        corrected_text: String,
    },
    AcceptedWithoutEdit {
        text: String,
    },
}

pub enum CandidateLabel {
    Approved { trust_score_bps: u16 },
    Rejected { reason: &'static str },
}

pub struct CandidateClassifier;

impl CandidateClassifier {
    pub fn classify(evidence: CandidateEvidence) -> CandidateLabel {
        match evidence {
            CandidateEvidence::PreeditCorrection {
                raw_text,
                corrected_text,
            } => {
                let raw_trimmed = raw_text.trim();
                let corrected_trimmed = corrected_text.trim();

                if raw_trimmed == corrected_trimmed {
                    CandidateLabel::Rejected {
                        reason: "unchanged_text",
                    }
                } else {
                    CandidateLabel::Approved {
                        trust_score_bps: 10_000,
                    }
                }
            }
            CandidateEvidence::AcceptedWithoutEdit { text } => {
                let text_trimmed = text.trim();

                if text_trimmed.is_empty() {
                    CandidateLabel::Rejected {
                        reason: "empty_text",
                    }
                } else {
                    CandidateLabel::Approved {
                        trust_score_bps: 6_000,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CandidateClassifier, CandidateEvidence, CandidateLabel};

    #[test]
    fn preedit_correction_is_approved_high_value_evidence() {
        let evidence = CandidateEvidence::PreeditCorrection {
            raw_text: "original text".into(),
            corrected_text: "corrected text".into(),
        };

        let label = CandidateClassifier::classify(evidence);

        assert!(matches!(
            label,
            CandidateLabel::Approved {
                trust_score_bps: 10_000
            }
        ));
    }

    #[test]
    fn accepted_without_edit_is_observed_but_lower_trust() {
        let evidence = CandidateEvidence::AcceptedWithoutEdit {
            text: "observed text".into(),
        };

        let label = CandidateClassifier::classify(evidence);

        assert!(matches!(
            label,
            CandidateLabel::Approved {
                trust_score_bps: 6_000
            }
        ));
    }

    #[test]
    fn unchanged_preedit_correction_is_rejected() {
        let evidence = CandidateEvidence::PreeditCorrection {
            raw_text: "same text".into(),
            corrected_text: "same text".into(),
        };

        let label = CandidateClassifier::classify(evidence);

        assert!(matches!(
            label,
            CandidateLabel::Rejected {
                reason: "unchanged_text"
            }
        ));
    }

    #[test]
    fn empty_accepted_text_is_rejected() {
        let evidence = CandidateEvidence::AcceptedWithoutEdit { text: "   ".into() };

        let label = CandidateClassifier::classify(evidence);

        assert!(matches!(
            label,
            CandidateLabel::Rejected {
                reason: "empty_text"
            }
        ));
    }
}
