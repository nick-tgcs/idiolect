//! End-to-end contract for `idiolect-trainerctl revalidate`: the spawned
//! binary against a real store + audio dir, dry-run by default, `--apply` to
//! write, `--json` for machine-readable output. The model defaults to the
//! bundled fixture model so the command works without flags on a dev machine.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioStorePort, MetadataStorePort};
use idiolect_test_support::fixtures::restart_traffic_fixture_16khz_mono;

fn fixture_root(tag: &str) -> PathBuf {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
    let root = env::temp_dir().join(format!(
        "idiolect-trainerctl-e2e-{tag}-{}-{}",
        std::process::id(),
        now.as_nanos()
    ));
    fs::create_dir_all(&root).expect("fixture root");
    root
}

fn seed(root: &std::path::Path) {
    let mut store =
        SqliteMetadataStore::open_path(root.join("idiolect.sqlite")).expect("store should open");
    store.migrate().expect("store should migrate");
    let session_id = store
        .create_session(Some("traffic"))
        .expect("session should be created");
    store
        .commit_session(session_id, "traffic", "commit-e2e")
        .expect("session should commit");
    let utterance_id = store
        .session_utterance_link_for_test(session_id)
        .expect("link should query")
        .expect("session should have an utterance")
        .utterance_id;
    let encoded = OpusCodec::new()
        .encode(&restart_traffic_fixture_16khz_mono())
        .expect("fixture should encode");
    FileAudioStore::new(root.join("audio"), root.join("decoded"))
        .write_source_audio("default", &utterance_id, &encoded)
        .expect("audio should store");
}

#[test]
fn the_binary_revalidates_dry_by_default_and_writes_with_apply() {
    let root = fixture_root("dry-then-apply");
    seed(&root);
    let db = root.join("idiolect.sqlite");
    let audio = root.join("audio");

    let dry = Command::new(env!("CARGO_BIN_EXE_idiolect-trainerctl"))
        .args([
            "revalidate",
            "--db",
            db.to_str().expect("utf8"),
            "--audio-root",
            audio.to_str().expect("utf8"),
            "--json",
        ])
        .output()
        .expect("binary should run");
    assert!(dry.status.success(), "stderr: {}", String::from_utf8_lossy(&dry.stderr));
    let report: serde_json::Value =
        serde_json::from_slice(&dry.stdout).expect("json report on stdout");
    assert_eq!(report["scanned"], 1, "{report}");
    assert_eq!(report["retranscribed"], 1, "{report}");
    assert_eq!(report["applied"], false, "{report}");

    // Dry by default: nothing was written.
    let mut store = SqliteMetadataStore::open_path(&db).expect("store should reopen");
    store.migrate().expect("store should migrate");
    let feed = store
        .training_candidates_for_manifest_v2("default")
        .expect("manifest feed should read");
    assert_eq!(feed[0].corrected_transcript, "traffic");

    let apply = Command::new(env!("CARGO_BIN_EXE_idiolect-trainerctl"))
        .args([
            "revalidate",
            "--db",
            db.to_str().expect("utf8"),
            "--audio-root",
            audio.to_str().expect("utf8"),
            "--apply",
            "--json",
        ])
        .output()
        .expect("binary should run");
    assert!(apply.status.success(), "stderr: {}", String::from_utf8_lossy(&apply.stderr));
    let report: serde_json::Value =
        serde_json::from_slice(&apply.stdout).expect("json report on stdout");
    assert_eq!(report["applied"], true, "{report}");

    let feed = store
        .training_candidates_for_manifest_v2("default")
        .expect("manifest feed should read");
    let lowered = feed[0].corrected_transcript.to_lowercase();
    assert!(
        lowered.contains("restart") && lowered.contains("traffic"),
        "--apply persists the repaired label: {feed:?}"
    );
}
