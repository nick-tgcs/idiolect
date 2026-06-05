use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_sqlite::repository::{
    AdapterRegistration, AdapterRegistrationInput, AdapterRegistryStatus,
};
use idiolect_adapter_sqlite::SqliteMetadataStore;

#[test]
fn adapter_registry_persists_current_previous_best_and_historical() {
    let path = db_path("registry-persistence");
    {
        let mut store = open_store(&path);
        store
            .register_adapter_candidate(registration(
                "base-adapter",
                "base-artifact-digest",
                "base-manifest-digest",
                "base-metric-report-digest",
                "adapters/base",
                -0.01,
            ))
            .expect("base adapter should register");
        store
            .promote_adapter("default", "base-adapter")
            .expect("base adapter should promote");
        store
            .register_adapter_candidate(registration(
                "personal-adapter",
                "personal-artifact-digest",
                "personal-manifest-digest",
                "personal-metric-report-digest",
                "adapters/personal",
                -0.08,
            ))
            .expect("personal adapter should register");
        store
            .promote_adapter("default", "personal-adapter")
            .expect("personal adapter should promote");
        store
            .register_adapter_candidate(registration(
                "rejected-adapter",
                "rejected-artifact-digest",
                "rejected-manifest-digest",
                "rejected-metric-report-digest",
                "adapters/rejected",
                0.02,
            ))
            .expect("rejected adapter should register");
        store
            .reject_adapter("default", "rejected-adapter", "general_wer_regression")
            .expect("rejected adapter should persist");
    }

    let store = open_store(&path);
    let snapshot = store
        .adapter_registry_snapshot("default")
        .expect("snapshot should load after restart");

    assert_eq!(
        snapshot.current_active_adapter_id(),
        Some("personal-adapter")
    );
    assert_eq!(snapshot.previous_active_adapter_id(), Some("base-adapter"));
    assert_eq!(
        snapshot.best_historical_adapter_id(),
        Some("personal-adapter")
    );
    assert_eq!(
        snapshot.status_for("rejected-adapter"),
        Some(AdapterRegistryStatus::Rejected)
    );
    assert_eq!(
        snapshot.historical_adapter_ids(),
        &["base-adapter".to_owned(), "personal-adapter".to_owned()]
    );

    cleanup(&path);
}

#[test]
fn promotion_is_atomic_on_storage_failure() {
    let path = db_path("registry-atomic-failure");
    let mut store = open_store(&path);
    store
        .register_adapter_candidate(registration(
            "base-adapter",
            "base-artifact-digest",
            "base-manifest-digest",
            "base-metric-report-digest",
            "adapters/base",
            -0.01,
        ))
        .expect("base adapter should register");
    store
        .promote_adapter("default", "base-adapter")
        .expect("base adapter should promote");

    let failed = store.promote_adapter("default", "missing-candidate");
    assert!(failed.is_err(), "missing candidate should fail promotion");

    let snapshot = store
        .adapter_registry_snapshot("default")
        .expect("snapshot should still load");
    assert_eq!(snapshot.current_active_adapter_id(), Some("base-adapter"));
    assert_eq!(snapshot.previous_active_adapter_id(), None);

    cleanup(&path);
}

#[test]
fn rollback_restores_previous_active_adapter_after_restart() {
    let path = db_path("registry-rollback-restart");
    {
        let mut store = open_store(&path);
        store
            .register_adapter_candidate(registration(
                "base-adapter",
                "base-artifact-digest",
                "base-manifest-digest",
                "base-metric-report-digest",
                "adapters/base",
                -0.01,
            ))
            .expect("base adapter should register");
        store
            .promote_adapter("default", "base-adapter")
            .expect("base adapter should promote");
        store
            .register_adapter_candidate(registration(
                "personal-adapter",
                "personal-artifact-digest",
                "personal-manifest-digest",
                "personal-metric-report-digest",
                "adapters/personal",
                -0.08,
            ))
            .expect("personal adapter should register");
        store
            .promote_adapter("default", "personal-adapter")
            .expect("personal adapter should promote");
    }

    let mut restarted = open_store(&path);
    restarted
        .rollback_adapter("default")
        .expect("rollback should restore previous adapter");
    let snapshot = restarted
        .adapter_registry_snapshot("default")
        .expect("snapshot should load after rollback");

    assert_eq!(snapshot.current_active_adapter_id(), Some("base-adapter"));
    assert_eq!(
        snapshot.status_for("personal-adapter"),
        Some(AdapterRegistryStatus::RolledBack)
    );

    cleanup(&path);
}

#[test]
fn deleted_sample_marks_derived_adapter() {
    let path = db_path("registry-deleted-sample");
    let mut store = open_store(&path);
    store
        .register_adapter_candidate(
            registration(
                "sample-derived-adapter",
                "artifact-digest",
                "manifest-digest-with-sample",
                "metric-report-digest",
                "adapters/sample-derived",
                -0.08,
            )
            .with_training_candidate_id(41),
        )
        .expect("adapter should register with manifest item link");

    store
        .mark_adapters_derived_from_deleted_sample("default", 41)
        .expect("deleted sample should mark derived adapters");

    let snapshot = store
        .adapter_registry_snapshot("default")
        .expect("snapshot should load");
    assert!(snapshot
        .entry("sample-derived-adapter")
        .expect("adapter should be present")
        .derived_from_deleted_sample());

    cleanup(&path);
}

fn registration(
    adapter_id: &str,
    artifact_digest: &str,
    manifest_digest: &str,
    metric_report_digest: &str,
    adapter_path: &str,
    personal_wer_delta: f64,
) -> AdapterRegistration {
    AdapterRegistration::new(AdapterRegistrationInput {
        user_id: "default".to_owned(),
        adapter_id: adapter_id.to_owned(),
        artifact_digest: artifact_digest.to_owned(),
        manifest_digest: manifest_digest.to_owned(),
        metric_report_digest: metric_report_digest.to_owned(),
        base_model: "whisper-medium-en".to_owned(),
        adapter_path: adapter_path.to_owned(),
        metrics: metrics(personal_wer_delta),
    })
}

fn open_store(path: &PathBuf) -> SqliteMetadataStore {
    let mut store = SqliteMetadataStore::open_path(path).expect("store should open");
    store.migrate().expect("store should migrate");
    store
}

fn db_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("idiolect-{name}-{}-{nonce}.sqlite", process::id()))
}

fn cleanup(path: &PathBuf) {
    let _ = fs::remove_file(path);
}

fn metrics(personal_wer_delta: f64) -> String {
    format!(
        "{{\"wer_personal_delta\":{personal_wer_delta},\"wer_general_delta\":0.0,\"proper_noun_accuracy_delta\":0.0}}"
    )
}
