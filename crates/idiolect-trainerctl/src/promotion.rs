use crate::{ArtifactCompatibility, EvaluationReport};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionDecision {
    Promote,
    Reject { reason: &'static str },
}

#[derive(Debug, Clone, Copy)]
pub struct PromotionPolicy {
    pub max_general_wer_delta: f32,
    pub max_hallucination_delta: f32,
    pub max_deletion_rate_delta: f32,
    pub max_p95_latency_delta_ms: i32,
    pub max_realtime_factor_delta: f32,
    pub min_personal_wer_improvement: f32,
    pub min_command_accuracy_delta: f32,
}

impl Default for PromotionPolicy {
    fn default() -> Self {
        Self {
            max_general_wer_delta: 0.0,
            max_hallucination_delta: 0.0,
            max_deletion_rate_delta: 0.0,
            max_p95_latency_delta_ms: 0,
            max_realtime_factor_delta: 0.0,
            min_personal_wer_improvement: -0.01,
            min_command_accuracy_delta: 0.0,
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

    if metric_deltas.proper_noun_accuracy_delta() < 0.0 {
        return PromotionDecision::Reject {
            reason: "proper_noun_accuracy_regression",
        };
    }

    if (metric_deltas.command_accuracy_delta() as f32) < policy.min_command_accuracy_delta {
        return PromotionDecision::Reject {
            reason: "command_accuracy_regression",
        };
    }

    if metric_deltas.hallucination_delta() as f32 > policy.max_hallucination_delta {
        return PromotionDecision::Reject {
            reason: "hallucination_regression",
        };
    }

    if metric_deltas.deletion_rate_delta() as f32 > policy.max_deletion_rate_delta {
        return PromotionDecision::Reject {
            reason: "deletion_rate_regression",
        };
    }

    if metric_deltas.p95_latency_delta_ms() > policy.max_p95_latency_delta_ms {
        return PromotionDecision::Reject {
            reason: "latency_regression",
        };
    }

    if metric_deltas.realtime_factor_delta() as f32 > policy.max_realtime_factor_delta {
        return PromotionDecision::Reject {
            reason: "realtime_factor_regression",
        };
    }

    PromotionDecision::Promote
}

#[derive(Debug, Clone, Default)]
struct AdapterState {
    active_adapter_id: Option<String>,
    rollback_adapter_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AdapterRegistry {
    adapters: HashMap<String, AdapterState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterRegistryError {
    ArtifactIncompatible,
    ArtifactDigestMismatch,
    ManifestDigestMismatch,
    MetricReportDigestMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackError {
    NoRollbackTarget,
}

impl AdapterRegistry {
    pub fn register_active(&mut self, user_id: &str, adapter_id: impl Into<String>) {
        let entry = self.adapters.entry(user_id.to_string()).or_default();
        if entry.active_adapter_id.is_none() {
            entry.active_adapter_id = Some(adapter_id.into());
        }
    }

    pub fn promote(
        &mut self,
        user_id: &str,
        adapter_id: impl Into<String>,
        manifest_digest: &str,
        report: &EvaluationReport,
        compatibility: &ArtifactCompatibility,
    ) -> Result<(), AdapterRegistryError> {
        if !compatibility.is_compatible() {
            return Err(AdapterRegistryError::ArtifactIncompatible);
        }
        if report.artifact_digest() != compatibility.artifact_digest() {
            return Err(AdapterRegistryError::ArtifactDigestMismatch);
        }
        if manifest_digest != report.manifest_digest()
            || manifest_digest != compatibility.manifest_digest()
        {
            return Err(AdapterRegistryError::ManifestDigestMismatch);
        }
        if report.metric_report_digest() != compatibility.metric_report_digest() {
            return Err(AdapterRegistryError::MetricReportDigestMismatch);
        }

        let entry = self.adapters.entry(user_id.to_string()).or_default();
        entry.rollback_adapter_id = entry.active_adapter_id.clone();
        entry.active_adapter_id = Some(adapter_id.into());
        Ok(())
    }

    pub fn rollback(&mut self, user_id: &str) -> Result<(), RollbackError> {
        let entry = self
            .adapters
            .get_mut(user_id)
            .ok_or(RollbackError::NoRollbackTarget)?;

        let previous = entry
            .rollback_adapter_id
            .take()
            .ok_or(RollbackError::NoRollbackTarget)?;

        entry.active_adapter_id = Some(previous);
        Ok(())
    }

    #[must_use]
    pub fn active_adapter_id(&self, user_id: &str) -> Option<&str> {
        self.adapters
            .get(user_id)
            .and_then(|entry| entry.active_adapter_id.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        evaluate_promotion, AdapterRegistry, EvaluationReport, PromotionDecision, PromotionPolicy,
        RollbackError,
    };
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

    #[test]
    fn rollback_restores_previous_active_adapter() {
        let mut registry = AdapterRegistry::default();
        let user_id = "default";

        registry.register_active(user_id, "old-model");
        registry
            .promote(
                user_id,
                "new-model",
                "manifest-digest",
                &report(-0.08, 0.0, 0.0, 0, 0.0),
                &compatibility(true),
            )
            .expect("promotion should record new active adapter");

        let rolled_back = registry.rollback(user_id);

        assert!(
            rolled_back.is_ok(),
            "rollback should succeed when previous adapter exists"
        );
        assert_eq!(registry.active_adapter_id(user_id), Some("old-model"));
    }

    #[test]
    fn rollback_without_previous_target_reports_error() {
        let mut registry = AdapterRegistry::default();
        registry.register_active("default", "active-model");

        let rolled_back = registry.rollback("default");

        assert!(matches!(rolled_back, Err(RollbackError::NoRollbackTarget)));
    }
}
