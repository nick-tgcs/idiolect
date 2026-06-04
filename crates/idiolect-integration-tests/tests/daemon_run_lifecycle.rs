use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{ClientHello, CommitPreedit, IpcMessage, PreeditUpdate};

#[test]
fn daemon_run_fixture_audio_preedit_commit_persists_audio_session_and_candidate() {
    let fixture = DaemonFixture::new("commit");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello();
    client.send(IpcMessage::StartRecording);
    client.expect_preedit("restart traffic");
    client.send(IpcMessage::CommitPreedit(CommitPreedit {
        text: "restart Traefik".to_owned(),
    }));
    drop(client);
    assert_daemon_exits_successfully(daemon);

    let store = open_store(&fixture.database_path());
    assert_eq!(store.training_candidate_count_for_test().unwrap(), 1);
    assert_eq!(store.event_count_for_test().unwrap(), 3);
    assert_eq!(audio_file_count(&fixture.audio_dir()), 1);
}

#[test]
fn daemon_run_cancel_does_not_commit_text() {
    let fixture = DaemonFixture::new("cancel");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello();
    client.send(IpcMessage::StartRecording);
    client.expect_preedit("restart traffic");
    client.send(IpcMessage::CancelPreedit);
    drop(client);
    assert_daemon_exits_successfully(daemon);

    let store = open_store(&fixture.database_path());
    assert_eq!(store.training_candidate_count_for_test().unwrap(), 0);
    assert_eq!(store.event_count_for_test().unwrap(), 2);
}

#[test]
fn daemon_run_retry_does_not_duplicate_committed_session() {
    let fixture = DaemonFixture::new("retry");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello();
    client.send(IpcMessage::StartRecording);
    client.expect_preedit("restart traffic");
    client.send(IpcMessage::StartRecording);
    client.expect_preedit("restart traffic");
    client.send(IpcMessage::CommitPreedit(CommitPreedit {
        text: "restart Traefik".to_owned(),
    }));
    drop(client);
    assert_daemon_exits_successfully(daemon);

    let store = open_store(&fixture.database_path());
    assert_eq!(store.training_candidate_count_for_test().unwrap(), 1);
    assert_eq!(audio_file_count(&fixture.audio_dir()), 2);
}

#[test]
fn daemon_disconnect_marks_session_abandoned() {
    let fixture = DaemonFixture::new("disconnect");
    let daemon = fixture.spawn_daemon();
    {
        let mut client = DaemonClient::connect(&fixture.socket_path());
        client.send_hello();
        client.send(IpcMessage::StartRecording);
        client.expect_preedit("restart traffic");
    }
    assert_daemon_exits_successfully(daemon);

    let store = open_store(&fixture.database_path());
    assert_eq!(store.training_candidate_count_for_test().unwrap(), 0);
    assert_eq!(store.event_count_for_test().unwrap(), 2);
}

#[test]
fn daemon_run_unsupported_asr_engine_returns_safe_error() {
    let fixture = DaemonFixture::new_with_asr("unsupported-asr", "unsupported-asr");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello();
    client.send(IpcMessage::StartRecording);
    let response = client.read();
    drop(client);
    assert_daemon_exits_successfully(daemon);

    match response {
        IpcMessage::Error(error) => {
            assert_eq!(error.code, "asr-unavailable");
            assert!(!error.message.contains("private daemon transcript"));
        }
        other => panic!("expected safe ASR error, got {other:?}"),
    }
}

struct DaemonFixture {
    root: PathBuf,
}

impl DaemonFixture {
    fn new(tag: &str) -> Self {
        Self::new_with_asr(tag, "fixture")
    }

    fn new_with_asr(tag: &str, asr_engine: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = env::temp_dir().join(format!(
            "idiolect-daemon-run-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let fixture = Self { root };
        fixture.write_config(asr_engine);
        fixture
    }

    fn spawn_daemon(&self) -> JoinHandle<Result<(), String>> {
        let args = vec![
            "run".to_owned(),
            "--config".to_owned(),
            self.config_path().to_string_lossy().into_owned(),
            "--shutdown-after-client".to_owned(),
        ];
        thread::spawn(move || {
            idiolectd::runtime::run_cli(&args)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    fn write_config(&self, asr_engine: &str) {
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
input_device = "fixture"
capture_sample_rate = 16000
processing_sample_rate = 16000
channels = 1

[vad]
engine = "webrtc"
threshold = 0.5
min_speech_ms = 250
pre_roll_ms = 300
post_roll_ms = 700
max_utterance_ms = 30000

[asr]
engine = "{asr_engine}"
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
private_text_probe = "private daemon transcript"

[observability]
log_private_text = false
"#,
                socket_path = self.socket_path().display(),
                data_dir = self.data_dir().display(),
                database_path = self.database_path().display(),
                asr_engine = asr_engine,
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

    fn audio_dir(&self) -> PathBuf {
        self.root.join("data").join("audio")
    }

    fn model_path(&self) -> PathBuf {
        self.root
            .join("data")
            .join("models")
            .join("whisper")
            .join("whisper-medium-en.bin")
    }
}

impl Drop for DaemonFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct DaemonClient {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl DaemonClient {
    fn connect(socket_path: &Path) -> Self {
        let stream = connect_client(socket_path);
        let reader = BufReader::new(stream.try_clone().expect("client stream should clone"));
        Self { stream, reader }
    }

    fn send_hello(&mut self) {
        self.send(IpcMessage::ClientHello(ClientHello {
            client_name: "idiolect-daemon-lifecycle-test".to_owned(),
            protocol_version: 1,
            features: vec!["preedit".to_owned(), "commit".to_owned()],
        }));
        match self.read() {
            IpcMessage::ServerHello(server) => assert_eq!(server.protocol_version, 1),
            other => panic!("expected ServerHello, got {other:?}"),
        }
    }

    fn send(&mut self, message: IpcMessage) {
        let line = encode_json_line(&message).expect("message should encode");
        self.stream
            .write_all(line.as_bytes())
            .expect("message should write");
        self.stream.flush().expect("message should flush");
    }

    fn read(&mut self) -> IpcMessage {
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .expect("message should read");
        assert!(read > 0, "server closed before sending a message");
        decode_json_line(&line).expect("message should decode")
    }

    fn expect_preedit(&mut self, expected: &str) {
        match self.read() {
            IpcMessage::PreeditUpdate(PreeditUpdate { text }) => assert_eq!(text, expected),
            other => panic!("expected PreeditUpdate, got {other:?}"),
        }
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

fn assert_daemon_exits_successfully(daemon: JoinHandle<Result<(), String>>) {
    match daemon.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("daemon returned error: {error}"),
        Err(_) => panic!("daemon thread panicked"),
    }
}

fn open_store(path: &Path) -> SqliteMetadataStore {
    let mut store = SqliteMetadataStore::open_path(path).expect("database should open");
    store.migrate().expect("migrations should run");
    store
}

fn audio_file_count(root: &Path) -> usize {
    let mut count = 0_usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("failed to read {}: {error}", path.display()),
        };
        for entry in entries {
            let entry = entry.expect("dir entry should read");
            let file_type = entry.file_type().expect("file type should read");
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file()
                && entry.path().extension().and_then(|ext| ext.to_str()) == Some("ogg")
            {
                count += 1;
            }
        }
    }
    count
}
