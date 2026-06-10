//! Contract tests for the daemon's CLI dispatch (`idiolectd::runtime::run_cli`)
//! and the observability redactor. These drive the public runtime surface
//! directly, in-process, to pin the argument parsing, error reporting, and the
//! fixture-once dictation path — the branches the subprocess smoke tests don't
//! reach. Every case here is side-effect-free except the fixture-once ones,
//! which write only to a unique temp database.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolectd::runtime::{redact_observability_line, redact_observability_line_for_test, run_cli};
use serde_json::Value;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn unique_temp_db_path(tag: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    env::temp_dir().join(format!(
        "idiolectd-cli-{tag}-{}-{}.db",
        std::process::id(),
        now.as_nanos()
    ))
}

// --------------------------------------------------------------------------
// Happy dispatch arms
// --------------------------------------------------------------------------

#[test]
fn version_json_reports_name_and_protocol() {
    let out = run_cli(&args(&["--version", "--json"])).expect("version should succeed");
    let json: Value = serde_json::from_str(&out).expect("version output should be json");
    assert_eq!(json["name"], "idiolectd");
    assert_eq!(json["protocol_version"], 1);
}

#[test]
fn config_print_default_emits_serializable_config() {
    let out =
        run_cli(&args(&["config", "print-default", "--json"])).expect("print-default should work");
    let json: Value = serde_json::from_str(&out).expect("config output should be json");
    assert!(json.is_object(), "default config should be a JSON object");
}

// --------------------------------------------------------------------------
// Usage / argument errors
// --------------------------------------------------------------------------

#[test]
fn empty_args_require_a_command() {
    let err = run_cli(&[]).expect_err("no command should error");
    assert!(err.to_string().contains("command is required"));
}

#[test]
fn unknown_command_is_reported() {
    let err = run_cli(&args(&["wibble"])).expect_err("unknown command should error");
    assert!(err.to_string().contains("unknown command: wibble"));
}

#[test]
fn run_without_config_flag_is_rejected() {
    let err = run_cli(&args(&["run"])).expect_err("run needs --config");
    assert!(err.to_string().contains("--config"));
}

#[test]
fn run_with_dangling_config_flag_is_rejected() {
    let err = run_cli(&args(&["run", "--config"])).expect_err("--config needs a value");
    assert!(err.to_string().contains("--config"));
}

#[test]
fn run_with_unknown_argument_is_rejected() {
    let err =
        run_cli(&args(&["run", "--config", "/tmp/x", "--nope"])).expect_err("unknown run arg");
    assert!(err.to_string().contains("unknown run argument: --nope"));
}

#[test]
fn run_with_missing_config_file_reports_io_error() {
    let missing = unique_temp_db_path("no-such-config");
    let err = run_cli(&args(&["run", "--config", missing.to_str().unwrap()]))
        .expect_err("missing config file should error");
    assert!(err.to_string().contains("read config"));
}

#[test]
fn run_with_unparseable_config_reports_parse_error() {
    let path = unique_temp_db_path("garbage-config");
    fs::write(&path, b"this is not valid toml = = =").expect("write garbage config");
    let err = run_cli(&args(&["run", "--config", path.to_str().unwrap()]))
        .expect_err("garbage config should error");
    assert!(err.to_string().contains("config parse failed"));
    let _ = fs::remove_file(&path);
}

// --------------------------------------------------------------------------
// fixture-once: argument validation + the dictation path
// --------------------------------------------------------------------------

#[test]
fn fixture_once_requires_exactly_one_of_commit_or_cancel() {
    let db = unique_temp_db_path("fixture-neither");
    let err = run_cli(&args(&[
        "fixture-once",
        "--db",
        db.to_str().unwrap(),
        "--transcript",
        "hello",
    ]))
    .expect_err("neither commit nor cancel should error");
    assert!(err.to_string().contains("exactly one of"));
}

#[test]
fn fixture_once_rejects_both_commit_and_cancel() {
    let db = unique_temp_db_path("fixture-both");
    let err = run_cli(&args(&[
        "fixture-once",
        "--db",
        db.to_str().unwrap(),
        "--transcript",
        "hello",
        "--commit",
        "--cancel",
    ]))
    .expect_err("both commit and cancel should error");
    assert!(err.to_string().contains("exactly one of"));
}

#[test]
fn fixture_once_requires_a_transcript() {
    let db = unique_temp_db_path("fixture-no-transcript");
    let err = run_cli(&args(&[
        "fixture-once",
        "--db",
        db.to_str().unwrap(),
        "--commit",
    ]))
    .expect_err("missing transcript should error");
    assert!(err.to_string().contains("--transcript"));
}

#[test]
fn fixture_once_commit_with_correction_records_text() {
    let db = unique_temp_db_path("fixture-commit");
    let out = run_cli(&args(&[
        "fixture-once",
        "--db",
        db.to_str().unwrap(),
        "--transcript",
        "restart traffic",
        "--corrected",
        "restart Traefik",
        "--commit",
    ]))
    .expect("fixture-once commit should succeed");
    let json: Value = serde_json::from_str(&out).expect("fixture-once output should be json");
    assert_eq!(json["committed"], true);
    assert_eq!(json["cancelled"], false);
    assert_eq!(json["text"], "restart Traefik");
    let _ = fs::remove_file(&db);
}

#[test]
fn fixture_once_cancel_records_no_commit() {
    let db = unique_temp_db_path("fixture-cancel");
    let out = run_cli(&args(&[
        "fixture-once",
        "--db",
        db.to_str().unwrap(),
        "--transcript",
        "open notes",
        "--cancel",
    ]))
    .expect("fixture-once cancel should succeed");
    let json: Value = serde_json::from_str(&out).expect("fixture-once output should be json");
    assert_eq!(json["committed"], false);
    assert_eq!(json["cancelled"], true);
    let _ = fs::remove_file(&db);
}

// --------------------------------------------------------------------------
// serve-fixture / serve-real-fixture: argument validation + early guards
// --------------------------------------------------------------------------

#[test]
fn serve_fixture_requires_a_socket() {
    let err = run_cli(&args(&[
        "serve-fixture",
        "--db",
        "/tmp/x.db",
        "--transcript",
        "hi",
    ]))
    .expect_err("serve-fixture needs --socket");
    assert!(err.to_string().contains("--socket"));
}

#[test]
fn serve_real_fixture_requires_all_paths() {
    let err = run_cli(&args(&[
        "serve-real-fixture",
        "--socket",
        "/tmp/s.sock",
        "--db",
        "/tmp/x.db",
    ]))
    .expect_err("serve-real-fixture needs the fixture paths");
    assert!(
        err.to_string().contains("--audio-fixture") || err.to_string().contains("--whisper-model")
    );
}

#[test]
fn serve_real_fixture_guards_missing_audio_fixture() {
    // All flags present, but the audio fixture file does not exist: the real
    // fixture path must fail fast before binding any socket.
    let err = run_cli(&args(&[
        "serve-real-fixture",
        "--socket",
        "/tmp/idiolect-test-never.sock",
        "--db",
        "/tmp/idiolect-test-never.db",
        "--audio-fixture",
        "/no/such/audio.wav",
        "--whisper-model",
        "/no/such/model.bin",
    ]))
    .expect_err("missing audio fixture should error");
    assert!(err.to_string().contains("audio fixture does not exist"));
}

// --------------------------------------------------------------------------
// Observability redaction
// --------------------------------------------------------------------------

#[test]
fn redaction_passes_everything_through_when_private_logging_is_on() {
    let line = "event=commit transcript=secret words text=more";
    assert_eq!(redact_observability_line(line, true), line);
}

#[test]
fn redaction_masks_each_sensitive_marker_when_private_logging_is_off() {
    for marker in [
        "transcript=",
        "raw_transcript=",
        "corrected_transcript=",
        "text=",
        "clipboard=",
    ] {
        let line = format!("event=x {marker}super secret value");
        let redacted = redact_observability_line(&line, false);
        assert_eq!(redacted, format!("event=x {marker}[redacted]"));
    }
}

#[test]
fn redaction_leaves_lines_without_markers_untouched() {
    let line = "event=heartbeat latency_ms=12";
    assert_eq!(redact_observability_line(line, false), line);
    // The test-only wrapper must behave identically to the real redactor.
    assert_eq!(
        redact_observability_line_for_test(line, false),
        redact_observability_line(line, false)
    );
}
