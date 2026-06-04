use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{ClientHello, IpcMessage, FEATURE_COMMIT, FEATURE_PREEDIT};

#[test]
fn idiolectd_run_starts_socket_and_accepts_hello() {
    let fixture = RunLoopFixture::new("hello");
    let mut daemon = fixture.spawn_daemon();
    let mut stream = connect_client(&fixture.socket_path());
    let mut reader = BufReader::new(stream.try_clone().expect("client stream should clone"));

    send_hello(&mut stream, &mut reader);

    drop(reader);
    drop(stream);
    assert_daemon_exits_successfully(&mut daemon);
    assert!(!fixture.socket_path().exists());
}

#[test]
fn idiolectd_run_rejects_second_instance_on_same_socket() {
    let fixture = RunLoopFixture::new("second-instance");
    let mut first = fixture.spawn_daemon();
    let stream = connect_client(&fixture.socket_path());

    let output = Command::new(env!("CARGO_BIN_EXE_idiolectd"))
        .args([
            "run",
            "--config",
            fixture
                .config_path()
                .to_str()
                .expect("config path should be utf8"),
            "--shutdown-after-client",
        ])
        .output()
        .expect("second idiolectd run should execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("socket") || stderr.contains("bind"));
    assert!(!stderr.contains("private probe text"));

    drop(stream);
    assert_daemon_exits_successfully(&mut first);
}

#[test]
fn idiolectd_run_shutdown_cleans_socket_file() {
    let fixture = RunLoopFixture::new("shutdown-cleanup");
    let mut daemon = fixture.spawn_daemon();
    let stream = connect_client(&fixture.socket_path());

    assert!(fixture.socket_path().exists());
    drop(stream);
    assert_daemon_exits_successfully(&mut daemon);
    assert!(!fixture.socket_path().exists());
}

struct RunLoopFixture {
    root: PathBuf,
}

impl RunLoopFixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = env::temp_dir().join(format!(
            "idiolectd-run-loop-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let fixture = Self { root };
        fixture.write_config();
        fixture
    }

    fn spawn_daemon(&self) -> Child {
        Command::new(env!("CARGO_BIN_EXE_idiolectd"))
            .args([
                "run",
                "--config",
                self.config_path()
                    .to_str()
                    .expect("config path should be utf8"),
                "--shutdown-after-client",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("idiolectd run should spawn")
    }

    fn write_config(&self) {
        fs::create_dir_all(self.model_path().parent().expect("model parent"))
            .expect("model parent should be created");
        fs::write(self.model_path(), b"dummy model").expect("dummy model should be written");
        fs::write(
            self.config_path(),
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
use_gpu = false
threads = 1

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
private_text_probe = "private probe text"

[observability]
log_private_text = false
"#,
                socket_path = self.socket_path().display(),
                data_dir = self.data_dir().display(),
                database_path = self.database_path().display(),
            ),
        )
        .expect("config should be written");
    }

    fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
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

impl Drop for RunLoopFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn connect_client(socket_path: &Path) -> UnixStream {
    for _ in 0..500 {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return stream,
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
    panic!("client could not connect to {}", socket_path.display());
}

fn send_hello(stream: &mut UnixStream, reader: &mut BufReader<UnixStream>) {
    send_message(
        stream,
        &IpcMessage::ClientHello(ClientHello {
            client_name: "idiolectd-run-loop-test".to_owned(),
            protocol_version: 1,
            features: vec![FEATURE_PREEDIT.to_owned(), FEATURE_COMMIT.to_owned()],
        }),
    );

    match read_message(reader) {
        IpcMessage::ServerHello(server) => {
            assert_eq!(server.protocol_version, 1);
            assert_eq!(
                server.accepted_features,
                vec![FEATURE_PREEDIT.to_owned(), FEATURE_COMMIT.to_owned()]
            );
        }
        other => panic!("expected ServerHello, got {other:?}"),
    }
}

fn send_message(stream: &mut UnixStream, message: &IpcMessage) {
    let line = encode_json_line(message).expect("message should encode");
    stream
        .write_all(line.as_bytes())
        .expect("message should write");
    stream.flush().expect("message should flush");
}

fn read_message(reader: &mut BufReader<UnixStream>) -> IpcMessage {
    let mut line = String::new();
    let read = reader.read_line(&mut line).expect("message should read");
    assert!(read > 0, "server closed before sending a message");
    decode_json_line(&line).expect("message should decode")
}

fn assert_daemon_exits_successfully(daemon: &mut Child) {
    for _ in 0..500 {
        match daemon.try_wait().expect("daemon wait should not fail") {
            Some(status) => {
                assert!(status.success(), "daemon exited with {status}");
                return;
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }

    let _ = daemon.kill();
    let _ = daemon.wait();
    panic!("daemon did not exit after client disconnect");
}
