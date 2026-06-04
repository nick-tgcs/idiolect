use crate::{ArtifactCompatibility, EvaluationReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionDecision {
    Promote,
    Reject { reason: &'static str },
}

#[derive(Debug, Clone, Copy)]
pub struct PromotionPolicy {
    pub max_general_wer_delta: f32,
    pub max_hallucination_delta: f32,
    pub max_p95_latency_delta_ms: i32,
    pub min_personal_wer_improvement: f32,
}

impl Default for PromotionPolicy {
    fn default() -> Self {
        Self {
            max_general_wer_delta: 0.0,
            max_hallucination_delta: 0.0,
            max_p95_latency_delta_ms: 0,
            min_personal_wer_improvement: -0.01,
        }
    }
}

pub fn evaluate_promotion(
    policy: &PromotionPolicy,
    report: &EvaluationReport,
    compatibility: &ArtifactCompatibility,
) -> PromotionDecision {
    if !compatibility.is_compatible() {
        return PromotionDecision::Reject {
            reason: "artifact_incompatible",
        };
    }

    let metric_deltas = report.metric_deltas();

    if metric_deltas.personal_wer_delta() as f32 > policy.min_personal_wer_improvement {
        return PromotionDecision::Reject {
            reason: "personal_wer_not_improved",
        };
    }

    if metric_deltas.general_wer_delta() as f32 > policy.max_general_wer_delta {
        return PromotionDecision::Reject {
            reason: "general_wer_regression",
        };
    }

    if metric_deltas.hallucination_delta() as f32 > policy.max_hallucination_delta {
        return PromotionDecision::Reject {
            reason: "hallucination_regression",
        };
    }

    if metric_deltas.p95_latency_delta_ms() > policy.max_p95_latency_delta_ms {
        return PromotionDecision::Reject {
            reason: "latency_regression",
        };
    }

    if metric_deltas.proper_noun_accuracy_delta() < 0.0 {
        return PromotionDecision::Reject {
            reason: "proper_noun_accuracy_regression",
        };
    }

    PromotionDecision::Promote
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{evaluate_promotion, EvaluationReport, PromotionDecision, PromotionPolicy};
    use crate::ArtifactCompatibility;

    fn report(
        personal_wer_delta: f64,
        general_wer_delta: f64,
        hallucination_delta: f64,
        p95_latency_delta_ms: i32,
        proper_noun_accuracy_delta: f64,
    ) -> EvaluationReport {
        let raw = json!({
            "artifact_digest": "artifact-digest",
            "manifest_digest": "manifest-digest",
            "metric_report_digest": "metric-report-digest",
            "metric_deltas": {
                "personal_wer_delta": personal_wer_delta,
                "general_wer_delta": general_wer_delta,
                "hallucination_delta": hallucination_delta,
                "p95_latency_delta_ms": p95_latency_delta_ms,
                "proper_noun_accuracy_delta": proper_noun_accuracy_delta,
            },
        });

        serde_json::from_value(raw).expect("evaluation report json should deserialize")
    }

    fn compatibility(runtime_compatible: bool) -> ArtifactCompatibility {
        ArtifactCompatibility::new(
            "artifact-digest",
            "manifest-digest",
            "metric-report-digest",
            "base-model-id",
            "adapter-format-v1",
            "runtime-format-v1",
            runtime_compatible,
        )
    }

    #[test]
    fn promote_when_personal_improves_and_general_quality_does_not_regress() {
        let decision = evaluate_promotion(
            &PromotionPolicy::default(),
            &report(-0.08, 0.0, 0.0, 0, 0.0),
            &compatibility(true),
        );

        assert_eq!(decision, PromotionDecision::Promote);
    }

    #[test]
    fn reject_when_general_wer_regresses() {
        let decision = evaluate_promotion(
            &PromotionPolicy::default(),
            &report(-0.08, 0.01, 0.0, 0, 0.0),
            &compatibility(true),
        );

        assert!(matches!(
            decision,
            PromotionDecision::Reject {
                reason: "general_wer_regression"
            }
        ));
    }

    #[test]
    fn reject_when_artifact_is_not_runtime_compatible() {
        let decision = evaluate_promotion(
            &PromotionPolicy::default(),
            &report(-0.08, 0.0, 0.0, 0, 0.0),
            &compatibility(false),
        );

        assert!(matches!(
            decision,
            PromotionDecision::Reject {
                reason: "artifact_incompatible"
            }
        ));
    }

    #[test]
    fn reject_when_personal_wer_does_not_improve_enough() {
        let decision = evaluate_promotion(
            &PromotionPolicy::default(),
            &report(-0.005, 0.0, 0.0, 0, 0.0),
            &compatibility(true),
        );

        assert!(matches!(
            decision,
            PromotionDecision::Reject {
                reason: "personal_wer_not_improved"
            }
        ));
    }

    #[test]
    fn reject_when_latency_regresses() {
        let decision = evaluate_promotion(
            &PromotionPolicy::default(),
            &report(-0.08, 0.0, 0.0, 1, 0.0),
            &compatibility(true),
        );

        assert!(matches!(
            decision,
            PromotionDecision::Reject {
                reason: "latency_regression"
            }
        ));
    }
}
