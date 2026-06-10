//! Pause-triggered live translation, end to end through the real daemon:
//! one recording containing speech–pause–speech must yield one translated
//! `PreeditUpdate` per pause-delimited snippet *while recording continues*,
//! not a single transcript on stop.
//!
//! Driven via the reserved `fixture-stream` device (a canned
//! speech–pause–speech clip served through the live polling path), the fixture
//! ASR engine, and an uppercase translator stub honouring the external-command
//! contract (`<command> <input_lang> <output_lang>`, text on stdin).

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{ClientHello, CommitPreedit, IpcMessage, RecordingStatus};

#[test]
fn each_pause_snippet_is_translated_and_pushed_mid_recording() {
    let fixture = DaemonFixture::new("snippets");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);

    // Two pause-delimited snippets arrive while the mic is still recording —
    // each independently transcribed ("restart traffic") and translated by the
    // stub ([sv>ja] + uppercase).
    client.expect_preedit("[sv>ja] RESTART TRAFFIC");
    client.expect_preedit("[sv>ja] RESTART TRAFFIC");

    // The engine auto-commits a snippet mid-recording. The mic is still open,
    // so this must NOT publish a recording=false transition (the engine would
    // go idle and drop every later snippet).
    client.send(IpcMessage::CommitPreedit(CommitPreedit {
        text: "[sv>ja] RESTART TRAFFIC".to_owned(),
    }));

    // Stop: the stream is fully drained (no tail snippet), so the very next
    // push is the stop's recording=false — anything earlier is the
    // commit-while-recording bug.
    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);
}

#[test]
fn streaming_translation_to_english_needs_no_command() {
    // The Whisper-task fast path contract at the daemon boundary: target "en"
    // with no configured command must still stream snippets (the fixture ASR
    // stands in for the engine's internal translate task).
    let fixture = DaemonFixture::new("entarget").with_translation_overrides("auto", "en", None);
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);
    client.expect_preedit("restart traffic");
    client.expect_preedit("restart traffic");

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);
}

#[test]
fn review_mode_routes_every_snippet_through_the_review_flag() {
    // Requirement: output honours the "Review before insert" option — when it is
    // on, each translated snippet tells the client to open its review dialog
    // (review=true) instead of committing directly.
    let fixture = DaemonFixture::new("review");
    fixture.seed_tray_setting("review_mode", "true");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);
    client.expect_preedit_with_review("[sv>ja] RESTART TRAFFIC", true);
    client.expect_preedit_with_review("[sv>ja] RESTART TRAFFIC", true);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);
}

struct DaemonFixture {
    root: PathBuf,
    input_language: String,
    output_language: String,
    /// `None` writes an uppercase translator stub; `Some("")` means no command.
    command_override: Option<String>,
}

impl DaemonFixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = env::temp_dir().join(format!(
            "idiolect-translation-streaming-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let fixture = Self {
            root,
            input_language: "sv".to_owned(),
            output_language: "ja".to_owned(),
            command_override: None,
        };
        fixture.write_files();
        fixture
    }

    fn with_translation_overrides(
        mut self,
        input: &str,
        output: &str,
        command: Option<&str>,
    ) -> Self {
        self.input_language = input.to_owned();
        self.output_language = output.to_owned();
        self.command_override = Some(command.unwrap_or("").to_owned());
        self.write_files();
        self
    }

    /// Persists a tray-settings override in the daemon's database before it
    /// starts, standing in for the corresponding tray click.
    fn seed_tray_setting(&self, key: &str, value: &str) {
        use idiolect_ports::storage::MetadataStorePort;
        fs::create_dir_all(self.database_path().parent().expect("db parent"))
            .expect("db parent should be created");
        let mut store =
            idiolect_adapter_sqlite::SqliteMetadataStore::open_path(self.database_path())
                .expect("seed store should open");
        store.migrate().expect("seed store should migrate");
        store
            .set_tray_setting(key, value)
            .expect("setting should persist");
    }

    fn spawn_daemon(&self) -> JoinHandle<Result<(), String>> {
        // Headless CI runs several daemons inside one process; skip the ksni tray
        // so their pid-keyed D-Bus registrations don't collide and reset clients.
        std::env::set_var("IDIOLECT_DISABLE_TRAY", "1");
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

    fn translator_command(&self) -> String {
        match &self.command_override {
            Some(command) => command.clone(),
            None => {
                let path = self.root.join("uppercase-translator.sh");
                fs::write(
                    &path,
                    "#!/bin/sh\nprintf '[%s>%s] ' \"$1\" \"$2\"; tr '[:lower:]' '[:upper:]'\n",
                )
                .expect("translator stub should be written");
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                    .expect("translator stub should be executable");
                path.to_string_lossy().into_owned()
            }
        }
    }

    fn write_files(&self) {
        fs::create_dir_all(self.model_path().parent().expect("model parent"))
            .expect("model parent should be created");
        fs::write(self.model_path(), b"dummy model").expect("dummy model should be written");
        let command = self.translator_command();
        fs::write(
            self.config_path(),
            format!(
                r#"[user]
default_user_id = "default"

[daemon]
socket_path = "{socket_path}"
log_level = "info"

[audio]
input_device = "fixture-stream"
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

[translation]
enabled = true
input_language = "{input_language}"
output_language = "{output_language}"
command = "{command}"

[observability]
log_private_text = false
"#,
                socket_path = self.socket_path().display(),
                data_dir = self.data_dir().display(),
                database_path = self.database_path().display(),
                input_language = self.input_language,
                output_language = self.output_language,
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
        let reader_stream = stream.try_clone().expect("client stream should clone");
        reader_stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .expect("read timeout should set");
        let reader = BufReader::new(reader_stream);
        Self { stream, reader }
    }

    fn send_hello_with_status(&mut self) {
        self.send(IpcMessage::ClientHello(ClientHello {
            client_name: "idiolect-translation-streaming-test".to_owned(),
            protocol_version: 1,
            features: vec![
                "preedit".to_owned(),
                "commit".to_owned(),
                "recording_status".to_owned(),
            ],
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

    fn expect_recording_status(&mut self, expected: bool) {
        match self.read() {
            IpcMessage::RecordingStatus(RecordingStatus { recording }) => {
                assert_eq!(recording, expected, "unexpected recording state");
            }
            other => panic!("expected RecordingStatus({expected}), got {other:?}"),
        }
    }

    fn expect_preedit(&mut self, expected: &str) {
        match self.read() {
            IpcMessage::PreeditUpdate(update) => assert_eq!(update.text, expected),
            other => panic!("expected PreeditUpdate({expected:?}), got {other:?}"),
        }
    }

    fn expect_preedit_with_review(&mut self, expected: &str, review: bool) {
        match self.read() {
            IpcMessage::PreeditUpdate(update) => {
                assert_eq!(update.text, expected);
                assert_eq!(update.review, review, "review flag mismatch");
            }
            other => panic!("expected PreeditUpdate({expected:?}), got {other:?}"),
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
