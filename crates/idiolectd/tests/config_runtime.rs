use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn idiolectd_config_print_default_reports_json_without_private_text() {
    let output = Command::new(env!("CARGO_BIN_EXE_idiolectd"))
        .args(["config", "print-default", "--json"])
        .output()
        .expect("idiolectd config print-default should execute");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be json");

    assert_eq!(json["user"]["default_user_id"], "default");
    assert_eq!(json["asr"]["model"], "whisper-medium-en");
    assert_eq!(json["observability"]["log_private_text"], false);
    assert!(!stdout.contains("raw transcript"));
    assert!(!stdout.contains("corrected transcript"));
    assert!(!stdout.contains("surrounding application text"));
}

#[test]
fn idiolectd_run_rejects_missing_model_path() {
    let fixture = RuntimeFixture::new("missing-model");
    let config_path = fixture.write_config("missing private transcript", false);

    let output = Command::new(env!("CARGO_BIN_EXE_idiolectd"))
        .args([
            "run",
            "--config",
            config_path.to_str().expect("utf8 path"),
            "--check-config",
        ])
        .output()
        .expect("idiolectd run should execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("ASR model path does not exist"));
    assert!(!stderr.contains("missing private transcript"));
}

#[test]
fn idiolectd_run_uses_configured_socket_and_database_paths() {
    let fixture = RuntimeFixture::new("configured-paths");
    let config_path = fixture.write_config("configured private transcript", true);

    let output = Command::new(env!("CARGO_BIN_EXE_idiolectd"))
        .args([
            "run",
            "--config",
            config_path.to_str().expect("utf8 path"),
            "--check-config",
        ])
        .output()
        .expect("idiolectd run should execute");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let json: Value = serde_json::from_str(&stdout).expect("stdout should be json");

    assert_eq!(
        json["socket_path"].as_str(),
        Some(fixture.socket_path().to_string_lossy().as_ref())
    );
    assert_eq!(
        json["database_path"].as_str(),
        Some(fixture.database_path().to_string_lossy().as_ref())
    );
    assert_eq!(
        json["model_path"].as_str(),
        Some(fixture.model_path().to_string_lossy().as_ref())
    );
    assert!(!stdout.contains("configured private transcript"));
}

#[test]
fn idiolectd_run_does_not_log_private_text_by_default() {
    let fixture = RuntimeFixture::new("private-text");
    let private_text = "customer secret phrase should never appear";
    let config_path = fixture.write_config(private_text, true);

    let output = Command::new(env!("CARGO_BIN_EXE_idiolectd"))
        .args([
            "run",
            "--config",
            config_path.to_str().expect("utf8 path"),
            "--check-config",
        ])
        .output()
        .expect("idiolectd run should execute");

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");

    assert!(output.status.success());
    assert!(!stdout.contains(private_text));
    assert!(!stderr.contains(private_text));
}

struct RuntimeFixture {
    root: PathBuf,
}

impl RuntimeFixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = env::temp_dir().join(format!(
            "idiolectd-config-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root should be created");
        Self { root }
    }

    fn write_config(&self, private_text: &str, create_model: bool) -> PathBuf {
        if create_model {
            if let Some(parent) = self.model_path().parent() {
                fs::create_dir_all(parent).expect("model parent should be created");
            }
            fs::write(self.model_path(), b"dummy model").expect("dummy model should be written");
        }

        let config_path = self.root.join("config.toml");
        fs::write(
            &config_path,
            format!(
                r#"[user]
default_user_id = "default"

[daemon]
socket_path = "{socket_path}"
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
use_gpu = true
threads = 8

[storage]
data_dir = "{data_dir}"
database_path = "{database_path}"
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
private_text_probe = "{private_text}"

[observability]
log_private_text = false
"#,
                socket_path = self.socket_path().display(),
                data_dir = self.data_dir().display(),
                database_path = self.database_path().display(),
                private_text = private_text,
            ),
        )
        .expect("config should be written");
        config_path
    }

    fn socket_path(&self) -> PathBuf {
        self.root.join("runtime").join("idiolect.sock")
    }

    fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    fn database_path(&self) -> PathBuf {
        self.root.join("data").join("db").join("idiolect.sqlite")
    }

    fn model_path(&self) -> PathBuf {
        self.root
            .join("data")
            .join("models")
            .join("whisper")
            .join("whisper-medium-en.bin")
    }
}

impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        remove_dir_all_if_exists(&self.root);
    }
}

fn remove_dir_all_if_exists(path: &Path) {
    match fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("failed to remove fixture root {}: {error}", path.display()),
    }
}
