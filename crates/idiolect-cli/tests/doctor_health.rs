use std::env;
use std::process::Command;

use serde_json::Value;

#[test]
fn doctor_json_reports_real_health_fields() {
    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args(["doctor", "--json"])
        .output()
        .expect("doctor command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let json: Value = serde_json::from_str(&stdout).expect("doctor stdout should be JSON");

    assert!(json.get("paths").is_some());
    assert!(json.get("sqlite_migrations").is_some());
    assert!(json.get("socket").is_some());
    assert!(json.get("model_file").is_some());
    assert!(json.get("fcitx5_metadata").is_some());
    assert_ne!(json.get("storage"), Some(&Value::String("ok".to_owned())));
    assert_ne!(json.get("ipc"), Some(&Value::String("ok".to_owned())));
}

#[test]
fn doctor_accepts_explicit_paths_for_deterministic_checks() {
    let temp = env::temp_dir().join(format!("idiolect-cli-doctor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).expect("temp root should be created");
    let model = temp.join("model.bin");
    std::fs::write(&model, b"model").expect("model should write");

    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args([
            "doctor",
            "--json",
            "--db",
            temp.join("missing.sqlite").to_str().expect("db path utf8"),
            "--socket",
            temp.join("missing.sock")
                .to_str()
                .expect("socket path utf8"),
            "--model",
            model.to_str().expect("model path utf8"),
            "--fcitx5-data-dir",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fcitx5/idiolect-fcitx5/data")
                .to_str()
                .expect("metadata path utf8"),
        ])
        .output()
        .expect("doctor command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let json: Value = serde_json::from_str(&stdout).expect("doctor stdout should be JSON");

    assert_eq!(json["model_file"]["status"], "present");
    assert_eq!(json["sqlite_migrations"]["status"], "missing");
    assert_eq!(json["socket"]["status"], "unreachable");
    assert_eq!(json["fcitx5_metadata"]["status"], "present");

    let _ = std::fs::remove_dir_all(&temp);
}
