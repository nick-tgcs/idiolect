#[path = "support/e2e.rs"]
mod e2e;
#[path = "support/e2e_fixture.rs"]
mod e2e_fixture;

use std::io::BufReader;

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ipc::messages::{CommitPreedit, IpcMessage};
use idiolect_trainerctl::{
    evaluate_promotion, AdapterRegistry, ArtifactCompatibility, CandidateClassifier,
    CandidateEvidence, EvaluationReport, LearningManifestBuilder, Manifest, ManifestBuildInput,
    ManifestCandidateInput, MetricDeltas, MetricDeltasInput, PromotionDecision, PromotionPolicy,
    RollbackError,
};

#[test]
fn candidate_capture_classifier_manifest_promotion_and_rollback_are_connected() {
    let paths = e2e::E2ePaths::new("learning-pipeline");
    populate_candidate(&paths);

    let store = e2e::open_store(&paths.db_path);
    let manifest = manifest_from_store(&store, "default");
    assert_eq!(manifest.candidates().len(), 1);
    assert_eq!(manifest.candidates()[0].trust_score_bps(), 10_000);

    let report = report_for_manifest(manifest.digest(), -0.08, 0.0, 0.0, 0, 0.0);
    let compatibility = compatibility_for_manifest(manifest.digest(), true);
    let decision = evaluate_promotion(&PromotionPolicy::default(), &report, &compatibility);
    assert_eq!(decision, PromotionDecision::Promote);

    let mut registry = AdapterRegistry::default();
    registry.register_active("default", "base-adapter");
    registry
        .promote(
            "default",
            "personal-adapter",
            manifest.digest(),
            &report,
            &compatibility,
        )
        .expect("promotion should record active adapter");
    assert_eq!(
        registry.active_adapter_id("default"),
        Some("personal-adapter")
    );

    registry
        .rollback("default")
        .expect("rollback should restore previous adapter");
    assert_eq!(registry.active_adapter_id("default"), Some("base-adapter"));

    paths.cleanup();
}

#[test]
fn deleted_user_data_is_excluded_from_manifest() {
    let paths = e2e::E2ePaths::new("learning-deleted-user");
    populate_candidate(&paths);

    let mut store = e2e::open_store(&paths.db_path);
    assert_eq!(manifest_from_store(&store, "default").candidates().len(), 1);

    store
        .delete_user_data_for_test("default")
        .expect("delete should succeed");

    let manifest = manifest_from_store(&store, "default");
    assert!(manifest.candidates().is_empty());

    paths.cleanup();
}

#[test]
fn promotion_matrix_rejects_each_regression_reason() {
    let policy = PromotionPolicy::default();
    let compatible = compatibility(true);
    let cases = [
        (
            report(-0.08, 0.01, 0.0, 0, 0.0),
            compatible.clone(),
            "general_wer_regression",
        ),
        (
            report(-0.08, 0.0, 0.01, 0, 0.0),
            compatible.clone(),
            "hallucination_regression",
        ),
        (
            report(-0.08, 0.0, 0.0, 1, 0.0),
            compatible.clone(),
            "latency_regression",
        ),
        (
            report(-0.08, 0.0, 0.0, 0, -0.01),
            compatible.clone(),
            "proper_noun_accuracy_regression",
        ),
        (
            report(-0.005, 0.0, 0.0, 0, 0.0),
            compatible.clone(),
            "personal_wer_not_improved",
        ),
        (
            report(-0.08, 0.0, 0.0, 0, 0.0),
            compatibility(false),
            "artifact_incompatible",
        ),
    ];

    for (report, compatibility, expected_reason) in cases {
        assert_eq!(
            evaluate_promotion(&policy, &report, &compatibility),
            PromotionDecision::Reject {
                reason: expected_reason
            }
        );
    }
}

#[test]
fn rollback_without_previous_adapter_reports_error() {
    let mut registry = AdapterRegistry::default();
    registry.register_active("default", "base-adapter");

    assert_eq!(
        registry.rollback("default"),
        Err(RollbackError::NoRollbackTarget)
    );
}

fn populate_candidate(paths: &e2e::E2ePaths) {
    let server = e2e_fixture::spawn_fixture_server(paths, "restart traffic");
    let mut stream = e2e::connect_client(&paths.socket_path);
    let mut reader = BufReader::new(stream.try_clone().expect("stream should clone"));

    e2e::send_hello(&mut stream, &mut reader);
    e2e::send_message(&mut stream, &IpcMessage::StartRecording);
    let _preedit = e2e::read_message(&mut reader);
    e2e::send_message(
        &mut stream,
        &IpcMessage::CommitPreedit(CommitPreedit {
            text: "restart Traefik".to_owned(),
        }),
    );
    drop(reader);
    drop(stream);
    server.join().expect("server thread should finish");
}

fn manifest_from_store(store: &SqliteMetadataStore, user_id: &str) -> Manifest {
    let inputs = store
        .training_candidates_for_manifest(user_id)
        .expect("manifest candidates should query")
        .into_iter()
        .map(|candidate| {
            let label = CandidateClassifier::classify(CandidateEvidence::PreeditCorrection {
                raw_text: candidate.raw_text.clone(),
                corrected_text: candidate.corrected_text.clone(),
            });
            ManifestCandidateInput::new(
                candidate.id.to_string(),
                candidate.raw_text,
                candidate.corrected_text,
                label,
            )
        })
        .collect::<Vec<_>>();

    LearningManifestBuilder::build(ManifestBuildInput::new(user_id, inputs))
        .expect("manifest should build")
}

fn report(
    personal_wer_delta: f64,
    general_wer_delta: f64,
    hallucination_delta: f64,
    p95_latency_delta_ms: i32,
    proper_noun_accuracy_delta: f64,
) -> EvaluationReport {
    report_for_manifest(
        "manifest-digest",
        personal_wer_delta,
        general_wer_delta,
        hallucination_delta,
        p95_latency_delta_ms,
        proper_noun_accuracy_delta,
    )
}

fn report_for_manifest(
    manifest_digest: &str,
    personal_wer_delta: f64,
    general_wer_delta: f64,
    hallucination_delta: f64,
    p95_latency_delta_ms: i32,
    proper_noun_accuracy_delta: f64,
) -> EvaluationReport {
    EvaluationReport::new(
        "artifact-digest",
        manifest_digest,
        "metric-report-digest",
        MetricDeltas::new(MetricDeltasInput {
            personal_wer_delta,
            general_wer_delta,
            cer_delta: 0.0,
            proper_noun_accuracy_delta,
            command_accuracy_delta: 0.0,
            hallucination_delta,
            deletion_rate_delta: 0.0,
            p95_latency_delta_ms,
            realtime_factor_delta: 0.0,
        }),
    )
}

fn compatibility(runtime_compatible: bool) -> ArtifactCompatibility {
    compatibility_for_manifest("manifest-digest", runtime_compatible)
}

fn compatibility_for_manifest(
    manifest_digest: &str,
    runtime_compatible: bool,
) -> ArtifactCompatibility {
    ArtifactCompatibility::new(
        "artifact-digest",
        manifest_digest,
        "metric-report-digest",
        "base-model-id",
        "adapter-format-v1",
        "runtime-format-v1",
        runtime_compatible,
    )
}
