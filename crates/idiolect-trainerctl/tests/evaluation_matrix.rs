use idiolect_trainerctl::{
    evaluate_promotion, ArtifactCompatibility, EvaluationReport, MetricDeltas, MetricDeltasInput,
    PromotionDecision, PromotionPolicy,
};

#[test]
fn evaluation_report_contains_master_plan_metrics() {
    let mut input = passing_metrics();
    input.cer_delta = -0.03;
    input.p95_latency_delta_ms = -12;
    input.realtime_factor_delta = -0.10;
    let report = report(input);
    let metrics = report.metric_deltas();

    assert_eq!(metrics.personal_wer_delta(), -0.08);
    assert_eq!(metrics.general_wer_delta(), 0.0);
    assert_eq!(metrics.cer_delta(), -0.03);
    assert_eq!(metrics.proper_noun_accuracy_delta(), 0.0);
    assert_eq!(metrics.command_accuracy_delta(), 0.0);
    assert_eq!(metrics.hallucination_delta(), 0.0);
    assert_eq!(metrics.deletion_rate_delta(), 0.0);
    assert_eq!(metrics.p95_latency_delta_ms(), -12);
    assert_eq!(metrics.realtime_factor_delta(), -0.10);
}

#[test]
fn promotion_rejects_command_regression() {
    let mut input = passing_metrics();
    input.command_accuracy_delta = -0.01;
    let decision = evaluate_promotion(
        &PromotionPolicy::default(),
        &report(input),
        &compatibility(),
    );

    assert_eq!(
        decision,
        PromotionDecision::Reject {
            reason: "command_accuracy_regression"
        }
    );
}

#[test]
fn promotion_rejects_deletion_rate_regression() {
    let mut input = passing_metrics();
    input.deletion_rate_delta = 0.01;
    let decision = evaluate_promotion(
        &PromotionPolicy::default(),
        &report(input),
        &compatibility(),
    );

    assert_eq!(
        decision,
        PromotionDecision::Reject {
            reason: "deletion_rate_regression"
        }
    );
}

#[test]
fn promotion_rejects_realtime_factor_regression() {
    let mut input = passing_metrics();
    input.realtime_factor_delta = 0.01;
    let decision = evaluate_promotion(
        &PromotionPolicy::default(),
        &report(input),
        &compatibility(),
    );

    assert_eq!(
        decision,
        PromotionDecision::Reject {
            reason: "realtime_factor_regression"
        }
    );
}

fn report(metric_deltas: MetricDeltasInput) -> EvaluationReport {
    EvaluationReport::new(
        "artifact-digest",
        "manifest-digest",
        "metric-report-digest",
        MetricDeltas::new(metric_deltas),
    )
}

fn passing_metrics() -> MetricDeltasInput {
    MetricDeltasInput {
        personal_wer_delta: -0.08,
        general_wer_delta: 0.0,
        cer_delta: 0.0,
        proper_noun_accuracy_delta: 0.0,
        command_accuracy_delta: 0.0,
        hallucination_delta: 0.0,
        deletion_rate_delta: 0.0,
        p95_latency_delta_ms: 0,
        realtime_factor_delta: 0.0,
    }
}

fn compatibility() -> ArtifactCompatibility {
    ArtifactCompatibility::new(
        "artifact-digest",
        "manifest-digest",
        "metric-report-digest",
        "base-model-id",
        "adapter-format-v1",
        "runtime-format-v1",
        true,
    )
}
