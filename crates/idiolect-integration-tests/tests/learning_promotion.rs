use idiolect_trainerctl::{
    evaluate_promotion, ArtifactCompatibility, EvaluationReport, PromotionDecision, PromotionPolicy,
};

#[test]
fn evaluate_promotion_with_passing_test_inputs_returns_promote() {
    let decision = evaluate_promotion(
        &PromotionPolicy::default(),
        &EvaluationReport::passing_for_test(),
        &ArtifactCompatibility::compatible_for_test(),
    );

    assert_eq!(decision, PromotionDecision::Promote);
}
