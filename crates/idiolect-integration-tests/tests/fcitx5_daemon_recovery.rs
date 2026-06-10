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
fn fcitx5_metadata_files_are_package_visible() {
    let addon = read_metadata("data/idiolect-addon.conf");
    assert!(addon.contains("Name=idiolect"));
    assert!(addon.contains("Library=idiolect"));

    let input_method = read_metadata("data/idiolect.conf");
    assert!(input_method.contains("Name=Idiolect"));
    assert!(input_method.contains("Addon=idiolect"));

    let metainfo = read_metadata("data/org.fcitx.Fcitx5.Addon.Idiolect.metainfo.xml");
    assert!(metainfo.contains("org.fcitx.Fcitx5.Addon.Idiolect"));
}

#[test]
fn fcitx5_reconnect_after_daemon_disconnect_starts_clean_session() {
    let fixture = RecoveryFixture::new("reconnect");

    let first_daemon = fixture.spawn_daemon();
    {
        let mut client = RecoveryClient::connect(&fixture.socket_path());
        client.send_hello();
        client.send(IpcMessage::StartRecording);
        client.expect_preedit("restart traffic");
    }
    assert_daemon_exits_successfully(first_daemon);

    let store = open_store(&fixture.database_path());
    assert_eq!(store.training_candidate_count_for_test().unwrap(), 0);
    assert_eq!(store.event_count_for_test().unwrap(), 2);

    let second_daemon = fixture.spawn_daemon();
    {
        let mut client = RecoveryClient::connect(&fixture.socket_path());
        client.send_hello();
        client.send(IpcMessage::StartRecording);
        client.expect_preedit("restart traffic");
        client.send(IpcMessage::CommitPreedit(CommitPreedit {
            text: "restart Traefik".to_owned(),
        }));
    }
    assert_daemon_exits_successfully(second_daemon);

    let store = open_store(&fixture.database_path());
    assert_eq!(store.training_candidate_count_for_test().unwrap(), 1);
}

fn read_metadata(relative_path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fcitx5/idiolect-fcitx5")
        .join(relative_path);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("metadata file {} should read: {error}", path.display()))
}

struct RecoveryFixture {
    root: PathBuf,
}

impl RecoveryFixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = env::temp_dir().join(format!(
            "idiolect-fcitx5-recovery-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let fixture = Self { root };
        fixture.write_config();
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
engine = "fixture"
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
private_text_probe = "private recovery text"

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

impl Drop for RecoveryFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct RecoveryClient {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl RecoveryClient {
    fn connect(socket_path: &Path) -> Self {
        let stream = connect_client(socket_path);
        let reader = BufReader::new(stream.try_clone().expect("client stream should clone"));
        Self { stream, reader }
    }

    fn send_hello(&mut self) {
        self.send(IpcMessage::ClientHello(ClientHello {
            client_name: "idiolect-fcitx5".to_owned(),
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
            IpcMessage::PreeditUpdate(PreeditUpdate { text, .. }) => assert_eq!(text, expected),
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
