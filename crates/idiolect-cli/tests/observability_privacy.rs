use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn doctor_audio_scope_reports_json_without_private_text() {
    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args(["doctor", "--audio", "--json"])
        .output()
        .expect("doctor command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let json: Value = serde_json::from_str(&stdout).expect("doctor stdout should be JSON");
    assert_eq!(json["audio"]["status"], "checked");
    assert!(!stdout.contains("private corrected transcript"));
}

#[test]
fn doctor_fcitx5_scope_reports_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args(["doctor", "--fcitx5", "--json"])
        .output()
        .expect("doctor command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let json: Value = serde_json::from_str(&stdout).expect("doctor stdout should be JSON");
    assert!(json.get("fcitx5").is_some());
}

#[test]
fn logs_show_redacts_private_text_by_default() {
    let fixture = LogFixture::new("redacted");
    let private = "private corrected transcript";
    fs::write(
        fixture.log_path(),
        format!("session=1 transcript={private}\nstorage=ok\n"),
    )
    .expect("log should write");

    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args([
            "logs",
            "show",
            "--log-file",
            fixture
                .log_path()
                .to_str()
                .expect("log path should be utf8"),
        ])
        .output()
        .expect("logs command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(!stdout.contains(private));
    assert!(stdout.contains("[redacted]"));
}

#[test]
fn logs_show_include_private_requires_explicit_flag() {
    let fixture = LogFixture::new("include-private");
    let private = "private corrected transcript";
    fs::write(fixture.log_path(), format!("transcript={private}\n")).expect("log should write");

    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args([
            "logs",
            "show",
            "--include-private",
            "--log-file",
            fixture
                .log_path()
                .to_str()
                .expect("log path should be utf8"),
        ])
        .output()
        .expect("logs command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains(private));
}

struct LogFixture {
    root: PathBuf,
}

impl LogFixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = env::temp_dir().join(format!(
            "idiolect-cli-logs-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root should be created");
        Self { root }
    }

    fn log_path(&self) -> PathBuf {
        self.root.join("idiolect.log")
    }
}

impl Drop for LogFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
