//! Covers the `run --check-config` preflight: parsing + validating the config,
//! resolving and creating the XDG-derived directory tree, and verifying the ASR
//! model is present — all without starting the daemon run loop. A temp `HOME`
//! isolates every XDG-derived path so the check never touches the real home.

use std::env;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolectd::runtime::run_cli;
use serde_json::Value;

fn config_toml(root: &Path) -> String {
    config_toml_with_socket(root, &root.join("runtime").join("idiolect.sock"))
}

fn config_toml_with_socket(root: &Path, socket: &Path) -> String {
    format!(
        r#"[user]
default_user_id = "default"

[daemon]
socket_path = "{socket}"
log_level = "info"

[audio]
input_device = "default"
capture_sample_rate = 48000
processing_sample_rate = 16000
channels = 1

[vad]
engine = "silero"
threshold = 0.5
min_speech_ms = 250
pre_roll_ms = 300
post_roll_ms = 700
max_utterance_ms = 30000

[asr]
engine = "whisper-rs"
model = "whisper-medium-en"
language = "en"
use_gpu = false
threads = 1

[storage]
data_dir = "{data}"
database_path = "{db}"
audio_codec = "opus"
audio_container = "ogg"
opus_bitrate_bps = 24000
high_value_opus_bitrate_bps = 32000

[training]
min_approved_examples = 50
trainer = "rust-native-lora"
auto_train = false

[privacy]
retain_audio = true
private_text_probe = "private probe text"

[observability]
log_private_text = false
"#,
        socket = socket.display(),
        data = root.join("data").display(),
        db = root
            .join("data")
            .join("db")
            .join("idiolect.sqlite")
            .display(),
    )
}

#[test]
fn run_check_config_prepares_paths_and_validates_model_presence() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    let root = env::temp_dir().join(format!(
        "idiolectd-check-config-{}-{}",
        std::process::id(),
        now.as_nanos()
    ));
    fs::create_dir_all(&root).expect("temp root");

    // Isolate every XDG-derived directory under a throwaway HOME.
    env::set_var("HOME", &root);
    for var in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        env::remove_var(var);
    }

    let model_path = root
        .join("data")
        .join("models")
        .join("whisper")
        .join("whisper-medium-en.bin");
    fs::create_dir_all(model_path.parent().expect("model parent")).expect("model dir");
    fs::write(&model_path, b"dummy model").expect("dummy model");

    let config_path = root.join("config.toml");
    fs::write(&config_path, config_toml(&root)).expect("config write");
    let config_arg = config_path.to_str().expect("utf8 config path").to_owned();

    let args = vec![
        "run".to_owned(),
        "--config".to_owned(),
        config_arg.clone(),
        "--check-config".to_owned(),
    ];

    let out = run_cli(&args).expect("check-config should succeed when the model is present");
    let json: Value = serde_json::from_str(&out).expect("check-config output should be json");
    assert_eq!(json["ready"], true);

    // Remove the model: the same preflight must now fail with a clear message.
    fs::remove_file(&model_path).expect("remove model");
    let err = run_cli(&args).expect_err("check-config should fail without the model");
    assert!(err.to_string().contains("ASR model path does not exist"));

    let _ = fs::remove_dir_all(&root);
}

/// End-to-end counterpart to the unit guard in `runtime::socket_guard_tests`:
/// a configured `daemon.socket_path` that overflows `sun_path` must be rejected
/// by the real CLI preflight (before any `bind`/EINVAL) with a readable message.
/// The accept path is the happy `--check-config` above (a short socket → ready).
/// No `HOME`/env mutation here, so it never races the env-dependent test.
#[test]
fn run_check_config_rejects_a_socket_path_that_overflows_sun_path() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    let root = env::temp_dir().join(format!(
        "idiolectd-overlong-socket-{}-{}",
        std::process::id(),
        now.as_nanos()
    ));
    fs::create_dir_all(&root).expect("temp root");

    // A 150-char socket filename overflows the limit (108 Linux / 104 macOS)
    // regardless of how short the temp root is — deterministic on any host.
    let overlong_socket = root.join(format!("{}.sock", "a".repeat(150)));
    let config_path = root.join("config.toml");
    fs::write(
        &config_path,
        config_toml_with_socket(&root, &overlong_socket),
    )
    .expect("config write");

    let args = vec![
        "run".to_owned(),
        "--config".to_owned(),
        config_path.to_str().expect("utf8 config path").to_owned(),
        "--check-config".to_owned(),
    ];

    let err = run_cli(&args).expect_err("an overlong socket path must be rejected");
    let message = err.to_string().to_lowercase();
    assert!(
        message.contains("socket path"),
        "names the cause: {message}"
    );
    assert!(
        message.contains("too long"),
        "explains the cause: {message}"
    );

    let _ = fs::remove_dir_all(&root);
}
