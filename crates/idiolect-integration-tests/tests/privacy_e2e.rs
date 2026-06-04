#[path = "support/e2e.rs"]
mod e2e;
#[path = "support/e2e_fixture.rs"]
mod e2e_fixture;

use std::io::BufReader;
use std::process::Command;

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ipc::messages::{CommitPreedit, IpcMessage};
use idiolect_trainerctl::{
    CandidateClassifier, CandidateEvidence, LearningManifestBuilder, Manifest, ManifestBuildInput,
    ManifestCandidateInput,
};

#[test]
fn privacy_delete_removes_training_data_and_future_manifest_excludes_user() {
    let paths = e2e::E2ePaths::new("privacy-delete");
    populate_candidate(&paths);

    let export_before = run_privacy_cli(&[
        "privacy",
        "export",
        "--user",
        "default",
        "--db",
        paths.db_path.to_str().expect("db path should be utf8"),
    ]);
    assert_success_json_contains(export_before, "\"training_candidates\":1");

    let store_before_delete = e2e::open_store(&paths.db_path);
    let manifest_before_delete = manifest_from_store(&store_before_delete, "default");
    assert_eq!(manifest_before_delete.candidates().len(), 1);
    assert_eq!(
        manifest_before_delete.candidates()[0].target_text(),
        "restart Traefik"
    );

    let delete_output = run_privacy_cli(&[
        "privacy",
        "delete",
        "--user",
        "default",
        "--db",
        paths.db_path.to_str().expect("db path should be utf8"),
        "--confirm-delete",
    ]);
    assert_success_json_contains(delete_output, "\"deleted\":true");

    let store_after_delete = e2e::open_store(&paths.db_path);
    assert_eq!(
        store_after_delete
            .training_candidate_count_for_test()
            .expect("candidate count should query"),
        0
    );
    assert_eq!(
        store_after_delete
            .user_data_deleted_event_count_for_test("default")
            .expect("deletion count should query"),
        1
    );

    let manifest_after_delete = manifest_from_store(&store_after_delete, "default");
    assert!(manifest_after_delete.candidates().is_empty());

    let export_after = run_privacy_cli(&[
        "privacy",
        "export",
        "--user",
        "default",
        "--db",
        paths.db_path.to_str().expect("db path should be utf8"),
    ]);
    assert_success_json_contains(export_after, "\"training_candidates\":0");

    paths.cleanup();
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
    let manifest_candidates = store
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

    LearningManifestBuilder::build(ManifestBuildInput::new(user_id, manifest_candidates))
        .expect("manifest should build")
}

fn run_privacy_cli(args: &[&str]) -> std::process::Output {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    Command::new(cargo)
        .args(["run", "-q", "-p", "idiolect-cli", "--"])
        .args(args)
        .output()
        .expect("idiolect-cli command should run through cargo")
}

fn assert_success_json_contains(output: std::process::Output, expected: &str) {
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(
        output.status.success(),
        "command should succeed; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains(expected),
        "stdout should contain {expected:?}, got {stdout:?}"
    );
}
