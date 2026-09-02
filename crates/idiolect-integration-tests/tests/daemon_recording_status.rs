//! The daemon is the single authority for recording state: it pushes
//! `RecordingStatus` to any client that negotiated the `recording_status` feature
//! — once right after the handshake and on every recording transition — so the
//! keyboard, the tray, and the adapter indicator can never disagree. Clients that
//! do not request the feature see the exact same byte stream as before.
//!
//! The live-capture toggle path (start → stop+transcribe) previously had no
//! integration coverage because it needs real hardware; these tests drive it
//! deterministically via the reserved `fixture-live` device.
//!
//! Coverage note (tray-menu freshness): the tray menu re-renders history on every
//! recording publication — including a commit/correction where the recording value
//! is unchanged (skipping that refresh made the menu lag a take behind, a field
//! bug). The render itself goes fire-and-forget into the ksni/StatusNotifier GUI,
//! which has no headless seam, so the end-to-end "menu shows the new entry" cannot
//! be asserted here. What pins the behaviour instead: `run_loop`'s
//! `status_push_is_edge_triggered_and_feature_gated` unit test (dedup lives ONLY on
//! the IPC push; the refresh is unconditional by construction) and
//! `commit_and_correction_push_no_duplicate_status` below (the wire stays clean).

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{
    ActivityPhase, ActivityStatus, ClientHello, CommitPreedit, IpcMessage, RecordingStatus,
    ReportCorrection,
};

#[test]
fn initial_recording_status_pushed_after_handshake() {
    let fixture = DaemonFixture::new("initial", "fixture");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    // Immediately after ServerHello the daemon syncs the authoritative state.
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);
}

#[test]
fn live_toggle_pushes_recording_true_then_false_around_the_take() {
    let fixture = DaemonFixture::new("toggle", "fixture-live");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false); // initial sync

    // One direction-free intent: the daemon decides this is a start.
    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);

    // Same intent again: the daemon decides this is a stop → transcribe, then
    // announces the mic is closed.
    client.send(IpcMessage::ToggleRecording);
    client.expect_preedit("restart traffic"); // the live preview snippet
    client.expect_preedit("restart traffic"); // the stop-time reconcile (verified text)
    client.expect_recording_status(false);

    // Committing the transcript is a text event, not a recording transition: no
    // redundant status push follows it.
    client.send(IpcMessage::CommitPreedit(CommitPreedit {
        text: "restart Traefik".to_owned(),
    }));
    drop(client);
    assert_daemon_exits_successfully(daemon);
}

#[test]
fn cancel_during_recording_pushes_recording_false() {
    let fixture = DaemonFixture::new("cancel", "fixture-live");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);

    // Cancel discards the take and releases the mic → recording false.
    client.send(IpcMessage::CancelPreedit);
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);
}

#[test]
fn commit_and_correction_push_no_duplicate_status() {
    // The stop already announced `recording: false`; the commit and a follow-up
    // correction change HISTORY (the tray menu re-renders) but not the recording
    // value, so neither may emit another RecordingStatus. The next message the
    // client sees must be the `true` of the next take — anything else is a
    // duplicate push.
    let fixture = DaemonFixture::new("nodup", "fixture-live");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);
    client.send(IpcMessage::ToggleRecording);
    client.expect_preedit("restart traffic"); // the live preview snippet
    client.expect_preedit("restart traffic"); // the stop-time reconcile (verified text)
    client.expect_recording_status(false);

    client.send(IpcMessage::CommitPreedit(CommitPreedit {
        text: "restart Traefik".to_owned(),
    }));
    client.send(IpcMessage::ReportCorrection(ReportCorrection {
        corrected_text: "restart the Traefik proxy".to_owned(),
    }));

    // Start the next take: the very next message must be its `true`.
    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);

    client.send(IpcMessage::CancelPreedit);
    client.expect_recording_status(false);
    drop(client);
    assert_daemon_exits_successfully(daemon);
}

#[test]
fn client_without_feature_sees_no_recording_status() {
    // Backward-compat guardrail: a client that does not request the feature gets
    // the exact pre-existing byte stream (PreeditUpdate only, no RecordingStatus).
    let fixture = DaemonFixture::new("nofeature", "fixture");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_legacy();
    client.send(IpcMessage::StartRecording);
    // The very next message must be the preedit, never a RecordingStatus.
    client.expect_preedit("restart traffic");
    client.send(IpcMessage::CommitPreedit(CommitPreedit {
        text: "restart Traefik".to_owned(),
    }));

    drop(client);
    assert_daemon_exits_successfully(daemon);
}

#[test]
fn the_decode_phase_is_announced_before_the_transcript_and_without_closing_the_take() {
    // The caret overlay's whole reason to exist. Between the microphone closing
    // and the transcript arriving the daemon decodes, which is where the user was
    // left staring at a badge that still said "listening". Two things are pinned
    // here: the phase IS announced in that gap, and `recording` does NOT move
    // with it — the engine only accepts a transcript while its take is open, so
    // flipping it early would silently drop the dictation.
    let fixture = DaemonFixture::new("activity", "fixture-live");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_activity();
    client.expect_recording_status(false); // initial sync
    client.expect_activity_status(ActivityPhase::Idle);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);
    client.expect_activity_status(ActivityPhase::Recording);

    // The stop: the decode phase lands FIRST, before any transcript, and carries
    // no RecordingStatus with it.
    client.send(IpcMessage::ToggleRecording);
    client.expect_activity_status(ActivityPhase::Transcribing);
    client.expect_preedit("restart traffic"); // the live preview snippet
    client.expect_preedit("restart traffic"); // the stop-time reconcile (verified text)
    client.expect_recording_status(false);
    client.expect_activity_status(ActivityPhase::Idle);

    drop(client);
    assert_daemon_exits_successfully(daemon);
}

#[test]
fn client_without_the_activity_feature_sees_no_activity_status() {
    // Strictly additive: an engine that predates the phase channel must get the
    // byte stream it always did, including through the decode gap.
    let fixture = DaemonFixture::new("noactivity", "fixture-live");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);
    client.send(IpcMessage::ToggleRecording);
    // No ActivityStatus anywhere: the next message is the transcript, as before.
    client.expect_preedit("restart traffic");
    client.expect_preedit("restart traffic");
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);
}

struct DaemonFixture {
    root: PathBuf,
    input_device: String,
}

impl DaemonFixture {
    fn new(tag: &str, input_device: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = env::temp_dir().join(format!(
            "idiolect-daemon-status-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let fixture = Self {
            root,
            input_device: input_device.to_owned(),
        };
        fixture.write_config();
        fixture
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
input_device = "{input_device}"
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
private_text_probe = "private daemon transcript"

[observability]
log_private_text = false
"#,
                socket_path = self.socket_path().display(),
                input_device = self.input_device,
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
        // A generous read timeout so a missing/incorrect push fails the test fast
        // instead of blocking forever.
        reader_stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .expect("read timeout should set");
        let reader = BufReader::new(reader_stream);
        Self { stream, reader }
    }

    fn send_hello_with_status(&mut self) {
        self.handshake(vec![
            "preedit".to_owned(),
            "commit".to_owned(),
            "recording_status".to_owned(),
            // Opt into the stop-time reconcile final (this client expects it).
            "reconcile".to_owned(),
        ]);
    }

    fn send_hello_with_activity(&mut self) {
        self.handshake(vec![
            "preedit".to_owned(),
            "commit".to_owned(),
            "recording_status".to_owned(),
            "reconcile".to_owned(),
            "activity_status".to_owned(),
        ]);
    }

    fn send_hello_legacy(&mut self) {
        self.handshake(vec!["preedit".to_owned(), "commit".to_owned()]);
    }

    fn handshake(&mut self, features: Vec<String>) {
        self.send(IpcMessage::ClientHello(ClientHello {
            client_name: "idiolect-recording-status-test".to_owned(),
            protocol_version: 1,
            features,
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

    fn expect_activity_status(&mut self, expected: ActivityPhase) {
        match self.read() {
            IpcMessage::ActivityStatus(ActivityStatus { phase }) => {
                assert_eq!(phase, expected, "unexpected take phase");
            }
            other => panic!("expected ActivityStatus({expected:?}), got {other:?}"),
        }
    }

    fn expect_preedit(&mut self, expected: &str) {
        match self.read() {
            IpcMessage::PreeditUpdate(update) => assert_eq!(update.text, expected),
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
