//! Integration test: the idiolect-ibus IPC client, driven against a REAL
//! idiolect daemon (run in-process on a thread, fixture audio device), proves
//! the learning loop end-to-end — a corrected commit becomes a stored training
//! candidate. No IBus / display needed.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ibus::ipc::{self, DaemonReader, DaemonSender};
use idiolect_ibus::session::DaemonClient;
use idiolect_ipc::IpcMessage;

#[test]
fn ibus_ipc_client_records_a_correction_as_training_candidate() {
    let fixture = Fixture::new("correction");
    let daemon = fixture.spawn_daemon();

    let (mut sender, mut reader) = connect_with_retry(&fixture.socket_path());

    // One direction-free toggle; the fixture daemon returns a deterministic draft.
    sender.toggle();
    assert_eq!(read_preedit(&mut reader), "restart traffic");

    // Commit a CORRECTED version — this is what the IBus engine sends after the
    // user edits the preedit.
    sender.commit("restart Traefik");

    drop(sender);
    drop(reader);
    assert_daemon_ok(daemon);

    let store = open_store(&fixture.database_path());
    assert_eq!(
        store.training_candidate_count_for_test().unwrap(),
        1,
        "a corrected commit should produce one training candidate"
    );
    assert_eq!(store.event_count_for_test().unwrap(), 3);
}

#[test]
fn ibus_ipc_client_cancel_records_nothing() {
    let fixture = Fixture::new("cancel");
    let daemon = fixture.spawn_daemon();

    let (mut sender, mut reader) = connect_with_retry(&fixture.socket_path());
    sender.toggle();
    assert_eq!(read_preedit(&mut reader), "restart traffic");
    sender.cancel();

    drop(sender);
    drop(reader);
    assert_daemon_ok(daemon);

    let store = open_store(&fixture.database_path());
    assert_eq!(store.training_candidate_count_for_test().unwrap(), 0);
}

fn read_preedit(reader: &mut DaemonReader) -> String {
    // The client negotiates `recording_status` and `activity_status`, so the
    // daemon interleaves status pushes (initial sync + transitions); skip them.
    loop {
        match reader.read_message().expect("daemon should send a message") {
            IpcMessage::PreeditUpdate(update) => return update.text,
            IpcMessage::RecordingStatus(_) | IpcMessage::ActivityStatus(_) => continue,
            other => panic!("expected PreeditUpdate, got {other:?}"),
        }
    }
}

fn connect_with_retry(socket_path: &Path) -> (DaemonSender, DaemonReader) {
    for _ in 0..500 {
        if let Ok((sender, reader, _reconcile)) = ipc::connect(socket_path) {
            return (sender, reader);
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("could not connect to daemon at {}", socket_path.display());
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = std::env::temp_dir().join(format!(
            "idiolect-ibus-ipc-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let fixture = Self { root };
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
private_text_probe = "private ibus transcript"

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

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assert_daemon_ok(daemon: JoinHandle<Result<(), String>>) {
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
