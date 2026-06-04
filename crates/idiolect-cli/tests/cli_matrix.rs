use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn doctor_requires_json() {
    let output = run_cli(["doctor"]);

    assert!(!output.status.success());
    assert_stderr_contains(output, "doctor requires --json");
}

#[test]
fn privacy_export_requires_user() {
    let db = unique_temp_db_path("export-requires-user");
    let output = run_cli([
        "privacy",
        "export",
        "--db",
        db.to_str().expect("temp db should be utf8"),
    ]);

    assert!(!output.status.success());
    assert_stderr_contains(output, "--user is required");
    cleanup_db_path(db);
}

#[test]
fn privacy_export_requires_db() {
    let output = run_cli(["privacy", "export", "--user", "default"]);

    assert!(!output.status.success());
    assert_stderr_contains(output, "--db is required");
}

#[test]
fn privacy_delete_requires_user() {
    let db = unique_temp_db_path("delete-requires-user");
    let output = run_cli([
        "privacy",
        "delete",
        "--db",
        db.to_str().expect("temp db should be utf8"),
        "--confirm-delete",
    ]);

    assert!(!output.status.success());
    assert_stderr_contains(output, "--user is required");
    cleanup_db_path(db);
}

#[test]
fn privacy_delete_requires_db() {
    let output = run_cli(["privacy", "delete", "--user", "default", "--confirm-delete"]);

    assert!(!output.status.success());
    assert_stderr_contains(output, "--db is required");
}

#[test]
fn privacy_delete_requires_confirm_delete() {
    let db = unique_temp_db_path("delete-requires-confirm");
    let output = run_cli([
        "privacy",
        "delete",
        "--user",
        "default",
        "--db",
        db.to_str().expect("temp db should be utf8"),
    ]);

    assert!(!output.status.success());
    assert_stderr_contains(output, "privacy delete requires --confirm-delete");
    cleanup_db_path(db);
}

#[test]
fn unknown_privacy_argument_fails() {
    let db = unique_temp_db_path("unknown-argument");
    let output = run_cli([
        "privacy",
        "export",
        "--user",
        "default",
        "--db",
        db.to_str().expect("temp db should be utf8"),
        "--surprise",
    ]);

    assert!(!output.status.success());
    assert_stderr_contains(output, "unknown privacy argument: --surprise");
    cleanup_db_path(db);
}

fn run_cli<const N: usize>(args: [&str; N]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args(args)
        .output()
        .expect("idiolect-cli command should run")
}

fn assert_stderr_contains(output: std::process::Output, expected: &str) {
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(
        stderr.contains(expected),
        "stderr should contain {expected:?}, got {stderr:?}"
    );
}

fn unique_temp_db_path(tag: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    env::temp_dir().join(format!(
        "idiolect-cli-matrix-{tag}-{}-{}.db",
        std::process::id(),
        now.as_nanos()
    ))
}

fn cleanup_db_path(path: PathBuf) {
    let _ = fs::remove_file(path);
}
