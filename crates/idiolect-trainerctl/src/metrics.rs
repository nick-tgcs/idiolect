use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationReport {
    artifact_digest: String,
    manifest_digest: String,
    metric_report_digest: String,
    metric_deltas: MetricDeltas,
}

impl EvaluationReport {
    #[must_use]
    pub fn passing_for_test() -> Self {
        Self::new(
            "artifact-digest",
            "manifest-digest",
            "metric-report-digest",
            MetricDeltas {
                personal_wer_delta: -0.02,
                general_wer_delta: 0.0,
                hallucination_delta: 0.0,
                p95_latency_delta_ms: 0,
                proper_noun_accuracy_delta: 0.0,
            },
        )
    }

    #[must_use]
    pub fn metric_deltas(&self) -> &MetricDeltas {
        &self.metric_deltas
    }

    #[must_use]
    pub fn new(
        artifact_digest: impl Into<String>,
        manifest_digest: impl Into<String>,
        metric_report_digest: impl Into<String>,
        metric_deltas: MetricDeltas,
    ) -> Self {
        Self {
            artifact_digest: artifact_digest.into(),
            manifest_digest: manifest_digest.into(),
            metric_report_digest: metric_report_digest.into(),
            metric_deltas,
        }
    }

    #[must_use]
    pub fn artifact_digest(&self) -> &str {
        &self.artifact_digest
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    #[must_use]
    pub fn metric_report_digest(&self) -> &str {
        &self.metric_report_digest
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricDeltas {
    personal_wer_delta: f64,
    general_wer_delta: f64,
    hallucination_delta: f64,
    p95_latency_delta_ms: i32,
    proper_noun_accuracy_delta: f64,
}

impl MetricDeltas {
    #[must_use]
    pub fn personal_wer_delta(&self) -> f64 {
        self.personal_wer_delta
    }

    #[must_use]
    pub fn general_wer_delta(&self) -> f64 {
        self.general_wer_delta
    }

    #[must_use]
    pub fn hallucination_delta(&self) -> f64 {
        self.hallucination_delta
    }

    #[must_use]
    pub fn p95_latency_delta_ms(&self) -> i32 {
        self.p95_latency_delta_ms
    }

    #[must_use]
    pub fn proper_noun_accuracy_delta(&self) -> f64 {
        self.proper_noun_accuracy_delta
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactCompatibility {
    artifact_digest: String,
    manifest_digest: String,
    metric_report_digest: String,
    base_model_id: String,
    adapter_format_version: String,
    runtime_format_version: String,
    runtime_compatible: bool,
}

impl ArtifactCompatibility {
    #[must_use]
    pub fn compatible_for_test() -> Self {
        Self::new(
            "artifact-digest",
            "manifest-digest",
            "metric-report-digest",
            "base-model-id",
            "adapter-format-v1",
            "runtime-format-v1",
            true,
        )
    }

    #[must_use]
    pub fn new(
        artifact_digest: impl Into<String>,
        manifest_digest: impl Into<String>,
        metric_report_digest: impl Into<String>,
        base_model_id: impl Into<String>,
        adapter_format_version: impl Into<String>,
        runtime_format_version: impl Into<String>,
        runtime_compatible: bool,
    ) -> Self {
        Self {
            artifact_digest: artifact_digest.into(),
            manifest_digest: manifest_digest.into(),
            metric_report_digest: metric_report_digest.into(),
            base_model_id: base_model_id.into(),
            adapter_format_version: adapter_format_version.into(),
            runtime_format_version: runtime_format_version.into(),
            runtime_compatible,
        }
    }

    #[must_use]
    pub fn is_compatible(&self) -> bool {
        !self.artifact_digest.is_empty()
            && !self.manifest_digest.is_empty()
            && !self.metric_report_digest.is_empty()
            && !self.base_model_id.is_empty()
            && !self.adapter_format_version.is_empty()
            && !self.runtime_format_version.is_empty()
            && self.runtime_compatible
    }

    #[must_use]
    pub fn artifact_digest(&self) -> &str {
        &self.artifact_digest
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    #[must_use]
    pub fn metric_report_digest(&self) -> &str {
        &self.metric_report_digest
    }

    #[must_use]
    pub fn base_model_id(&self) -> &str {
        &self.base_model_id
    }

    #[must_use]
    pub fn adapter_format_version(&self) -> &str {
        &self.adapter_format_version
    }

    #[must_use]
    pub fn runtime_format_version(&self) -> &str {
        &self.runtime_format_version
    }

    #[must_use]
    pub fn runtime_compatible(&self) -> bool {
        self.runtime_compatible
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{ArtifactCompatibility, EvaluationReport};

    #[test]
    fn metric_report_round_trips_and_preserves_delta_signs() {
        let raw = json!({
            "artifact_digest": "artifact-digest",
            "manifest_digest": "manifest-digest",
            "metric_report_digest": "metric-report-digest",
            "metric_deltas": {
                "personal_wer_delta": -0.12,
                "general_wer_delta": 0.0,
                "hallucination_delta": 0.1,
                "p95_latency_delta_ms": -5,
                "proper_noun_accuracy_delta": 0.2,
            },
        });

        let report: EvaluationReport =
            serde_json::from_value(raw).expect("metric json should deserialize");
        let serialized = serde_json::to_string(&report).expect("metric report should serialize");
        let round_tripped: EvaluationReport =
            serde_json::from_str(&serialized).expect("metric report should round trip");

        assert_eq!(round_tripped.metric_deltas().personal_wer_delta(), -0.12);
        assert_eq!(round_tripped.metric_deltas().general_wer_delta(), 0.0);
        assert_eq!(round_tripped.metric_deltas().p95_latency_delta_ms(), -5);
    }

    #[test]
    fn artifact_compatibility_requires_base_model_runtime_and_digests() {
        let compatible = ArtifactCompatibility::new(
            "artifact-digest",
            "manifest-digest",
            "metric-report-digest",
            "base-model-id",
            "adapter-format-v1",
            "runtime-format-v1",
            true,
        );

        assert!(compatible.is_compatible());

        let missing_artifact_digest = ArtifactCompatibility::new(
            "",
            "manifest-digest",
            "metric-report-digest",
            "base-model-id",
            "adapter-format-v1",
            "runtime-format-v1",
            true,
        );
        assert!(!missing_artifact_digest.is_compatible());

        let missing_manifest_digest = ArtifactCompatibility::new(
            "artifact-digest",
            "",
            "metric-report-digest",
            "base-model-id",
            "adapter-format-v1",
            "runtime-format-v1",
            true,
        );
        assert!(!missing_manifest_digest.is_compatible());

        let missing_metric_report_digest = ArtifactCompatibility::new(
            "artifact-digest",
            "manifest-digest",
            "",
            "base-model-id",
            "adapter-format-v1",
            "runtime-format-v1",
            true,
        );
        assert!(!missing_metric_report_digest.is_compatible());

        let missing_base_model_id = ArtifactCompatibility::new(
            "artifact-digest",
            "manifest-digest",
            "metric-report-digest",
            "",
            "adapter-format-v1",
            "runtime-format-v1",
            true,
        );
        assert!(!missing_base_model_id.is_compatible());

        let missing_adapter_format_version = ArtifactCompatibility::new(
            "artifact-digest",
            "manifest-digest",
            "metric-report-digest",
            "base-model-id",
            "",
            "runtime-format-v1",
            true,
        );
        assert!(!missing_adapter_format_version.is_compatible());

        let missing_runtime_format_version = ArtifactCompatibility::new(
            "artifact-digest",
            "manifest-digest",
            "metric-report-digest",
            "base-model-id",
            "adapter-format-v1",
            "",
            true,
        );
        assert!(!missing_runtime_format_version.is_compatible());

        let runtime_incompatible = ArtifactCompatibility::new(
            "artifact-digest",
            "manifest-digest",
            "metric-report-digest",
            "base-model-id",
            "adapter-format-v1",
            "runtime-format-v1",
            false,
        );
        assert!(!runtime_incompatible.is_compatible());
    }
}
