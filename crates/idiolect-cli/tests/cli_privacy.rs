use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn privacy_delete_requires_explicit_confirm_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args(["privacy", "delete", "--user", "default"])
        .output()
        .expect("privacy delete command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("--confirm-delete"));
}

#[test]
fn privacy_export_reports_json_user() {
    let path = unique_temp_db_path("privacy-export");
    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args([
            "privacy",
            "export",
            "--user",
            "default",
            "--db",
            path.to_str().expect("temp db path should be utf8"),
        ])
        .output()
        .expect("privacy export command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("\"user\":\"default\""));

    cleanup_db_path(path);
}

fn unique_temp_db_path(tag: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    env::temp_dir().join(format!(
        "idiolect-cli-{tag}-{}-{}.db",
        std::process::id(),
        now.as_nanos()
    ))
}

fn cleanup_db_path(path: PathBuf) {
    let _ = fs::remove_file(path);
}
