//! Value-type contract for the evaluation metric carriers. These are plain
//! data holders shared across the trainer pipeline; the test pins their
//! constructors and accessors so refactors can't silently drop a field.

use idiolect_ml_core::{EvaluationReport, EvaluationSuites};

#[test]
fn evaluation_suites_round_trips_its_ids() {
    let suites = EvaluationSuites::new(vec!["wer".to_owned(), "cer".to_owned()]);
    assert_eq!(suites.suite_ids(), ["wer", "cer"]);
    // Equality + clone are derived and used when comparing manifests.
    assert_eq!(suites.clone(), suites);
}

#[test]
fn evaluation_report_exposes_both_digests() {
    let report = EvaluationReport::new("sha256:report", "sha256:artifact");
    assert_eq!(report.digest(), "sha256:report");
    assert_eq!(report.artifact_digest(), "sha256:artifact");
    assert_ne!(
        report,
        EvaluationReport::new("sha256:other", "sha256:artifact")
    );
}
