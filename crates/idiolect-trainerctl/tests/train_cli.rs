//! End-to-end contract for `idiolect-trainerctl train`: real store, real audio,
//! the real bundled whisper fixture model — one short LoRA run that must emit
//! a loadable merged ggml artifact and a JSON report, and must NOT apply
//! anything (the artifact is a file; the daemon's config is untouched).

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
use idiolect_trainer_burn::ggml::GgmlModel;

fn fixture_root(tag: &str) -> PathBuf {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
    let root = env::temp_dir().join(format!(
        "idiolect-train-e2e-{tag}-{}-{}",
        std::process::id(),
        now.as_nanos()
    ));
    fs::create_dir_all(&root).expect("fixture root");
    root
}

fn base_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/whisper/ggml-tiny.en.bin")
}

fn seed_take(store: &mut SqliteMetadataStore, audio_store: &FileAudioStore, text: &str) {
    let session_id = store
        .create_session(Some(text))
        .expect("session should be created");
    store
        .commit_session(session_id, text, &format!("commit-{text}"))
        .expect("session should commit");
    let utterance_id = store
        .session_utterance_link_for_test(session_id)
        .expect("link should query")
        .expect("session should have an utterance")
        .utterance_id;
    let encoded = OpusCodec::new()
        .encode(&restart_traffic_fixture_16khz_mono())
        .expect("fixture should encode");
    audio_store
        .write_source_audio("default", &utterance_id, &encoded)
        .expect("audio should store");
}

#[test]
fn training_emits_a_loadable_merged_artifact_without_applying_it() {
    let root = fixture_root("emit");
    let db = root.join("idiolect.sqlite");
    {
        let mut store = SqliteMetadataStore::open_path(&db).expect("store should open");
        store.migrate().expect("store should migrate");
        let audio_store = FileAudioStore::new(root.join("audio"), root.join("decoded"));
        seed_take(&mut store, &audio_store, "restart traffic");
        seed_take(&mut store, &audio_store, "restart traffic please");
    }
    let output = root.join("personal.bin");

    let run = Command::new(env!("CARGO_BIN_EXE_idiolect-trainerctl"))
        .args([
            "train",
            "--db",
            db.to_str().expect("utf8"),
            "--audio-root",
            root.join("audio").to_str().expect("utf8"),
            "--base-model",
            base_model_path().to_str().expect("utf8"),
            "--output",
            output.to_str().expect("utf8"),
            "--epochs",
            "1",
            "--max-samples",
            "2",
        ])
        .output()
        .expect("binary should run");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&run.stdout).expect("json report on stdout");
    assert_eq!(report["usable_samples"], 2, "{report}");
    assert_eq!(report["applied"], false, "{report}");
    assert!(
        report["last_epoch_mean_loss"].as_f64().expect("loss") > 0.0,
        "{report}"
    );

    // The artifact is a structurally valid whisper model with the base dims.
    let merged = GgmlModel::read_file(&output).expect("artifact should parse as ggml");
    let base = GgmlModel::read_file(&base_model_path()).expect("base parses");
    assert_eq!(merged.hparams, base.hparams);
    assert_eq!(merged.tensors.len(), base.tensors.len());
}

/// Builds WITHOUT the `cuda` feature must reject `--gpu` with a clear error
/// rather than silently training on the CPU. (Compiled out on cuda builds —
/// there the flag is real; actual GPU execution has no headless CI seam.)
#[cfg(not(feature = "cuda"))]
#[test]
fn gpu_flag_on_a_cpu_only_build_fails_with_guidance() {
    let root = fixture_root("gpu-refused");
    let run = Command::new(env!("CARGO_BIN_EXE_idiolect-trainerctl"))
        .args([
            "train",
            "--db",
            root.join("idiolect.sqlite").to_str().expect("utf8"),
            "--audio-root",
            root.join("audio").to_str().expect("utf8"),
            "--base-model",
            base_model_path().to_str().expect("utf8"),
            "--output",
            root.join("out.bin").to_str().expect("utf8"),
            "--gpu",
        ])
        .output()
        .expect("binary should run");
    assert!(!run.status.success(), "--gpu must not silently fall back to CPU");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("--features cuda"),
        "the error must say how to fix it: {stderr}"
    );
}
