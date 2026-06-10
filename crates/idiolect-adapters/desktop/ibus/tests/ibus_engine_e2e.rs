//! Real end-to-end tests: the IBus **engine binary**, driven over D-Bus exactly
//! as the IBus input context would, proving text actually reaches the app via a
//! `CommitText` signal. Compiled only with the `ibus-engine` feature (like the
//! binary they drive). Two tests, two infra needs:
//!
//! - `engine_inserts_history_text_on_daemon_request` (the history-Insert path)
//!   drives the engine against a fake-daemon socket AND spawns its **own** private
//!   `dbus-daemon`, so it is fully self-contained — a normal `#[test]` (no
//!   `#[ignore]`, no `dbus-run-session`) that runs in the standard flow with
//!   `cargo test -p idiolect-ibus --features ibus-engine`.
//! - `engine_dictates_and_daemon_records_the_session` additionally spawns the real
//!   daemon (whose KSNI tray needs a session bus to attempt registration — it
//!   degrades gracefully when no StatusNotifier host answers), so it needs an
//!   **ambient session bus** and stays an `#[ignore]` manual test (run it with
//!   `-- --ignored engine_dictates`). It drives the live toggle path: the
//!   `fixture-live` device holds the "mic" open between two Super+T presses, so
//!   the daemon pushes `recording=true` before delivering the transcript —
//!   exactly the contract the engine's no-optimistic-flip state machine relies
//!   on. It proves the full dictation + correction → training-candidate chain.
//!
//! CI runs the self-contained one via `ci/scripts/test-ibus-e2e.sh` (the `e2e` job).
#![cfg(feature = "ibus-engine")]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{InsertText, ServerHello, PROTOCOL_VERSION};
use idiolect_ipc::IpcMessage;
use zbus::zvariant::{OwnedObjectPath, Value};
use zbus::Proxy;

const BUS_NAME: &str = "org.freedesktop.IBus.Idiolect";
const FACTORY_PATH: &str = "/org/freedesktop/IBus/Factory";
const FACTORY_IFACE: &str = "org.freedesktop.IBus.Factory";
const ENGINE_IFACE: &str = "org.freedesktop.IBus.Engine";
const MOD4: u32 = 1 << 6;
const KEY_T: u32 = 0x74;
const KEY_BACKSPACE: u32 = 0xff08;
const DRAFT: &str = "restart traffic";
const CORRECTED: &str = "restart Traefik";

#[tokio::test]
#[ignore = "needs an ambient session bus + the engine feature"]
async fn engine_dictates_and_daemon_records_the_session() {
    let fixture = Fixture::new("e2e");
    let daemon = fixture.spawn_daemon();
    // Killed on drop, so a panic mid-test never leaks the engine process.
    let engine = fixture.spawn_engine();
    wait_for_socket(&fixture.socket_path());

    let conn = zbus::Connection::session()
        .await
        .expect("ambient session bus");

    let factory = await_factory(&conn).await;
    let engine_path: OwnedObjectPath = factory
        .call("CreateEngine", &("idiolect",))
        .await
        .expect("CreateEngine");

    let engine_proxy = Proxy::new(&conn, BUS_NAME, &engine_path, ENGINE_IFACE)
        .await
        .expect("engine proxy");
    let mut commit_signals = engine_proxy
        .receive_signal("CommitText")
        .await
        .expect("subscribe CommitText");

    // Super+T -> the daemon opens the fixture-live "mic" and pushes
    // recording=true (the engine never flips its phase optimistically, so this
    // push is what arms it to accept the transcript).
    process_key(&engine_proxy, KEY_T, MOD4).await;
    // Super+T again -> the daemon stops the take, transcribes the deterministic
    // clip to DRAFT, and delivers it; the engine commits it straight into the
    // focused app — no preedit, no Enter. (The socket orders recording=true
    // before the transcript, so there is no race to wait out.)
    process_key(&engine_proxy, KEY_T, MOD4).await;
    let committed = {
        let msg = next_signal(&mut commit_signals).await;
        let body = msg.body();
        let (text,): (Value<'_>,) = body.deserialize().expect("commit body");
        extract_ibus_text(&text)
    };
    assert_eq!(
        committed, DRAFT,
        "the transcript is committed straight into the app"
    );

    // Fix it in place: backspace "traffic" and retype "Traefik". These pass
    // through to the app, but the engine tracks the tail edit.
    for _ in 0.."traffic".len() {
        process_key(&engine_proxy, KEY_BACKSPACE, 0).await;
    }
    for ch in "Traefik".chars() {
        process_key(&engine_proxy, ch as u32, 0).await;
    }
    // Reset closes the correction window -> the engine reports the correction.
    engine_proxy
        .call::<_, _, ()>("Reset", &())
        .await
        .expect("Reset");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Tear down: kill the engine (closing its daemon connection) before joining
    // the daemon, which exits once its single client disconnects.
    drop(engine);
    drop(conn);
    assert_daemon_ok(daemon);

    // The REAL daemon recorded the in-place fix as a raw->corrected signal —
    // still one candidate, now amended rather than `accepted_without_edit`.
    let store = open_store(&fixture.database_path());
    assert_eq!(store.training_candidate_count_for_test().unwrap(), 1);
    assert_eq!(
        store.latest_training_candidate_for_test().unwrap().unwrap(),
        (
            DRAFT.to_owned(),
            CORRECTED.to_owned(),
            "accepted_with_edit".to_owned()
        ),
        "the in-place correction feeds the learning loop"
    );
}

/// History "Insert" e2e: the daemon sends the engine an `InsertText` (as it does
/// when the user clicks tray "Insert"), and the engine types it straight into the
/// focused app — proven by the `CommitText` D-Bus signal it emits.
///
/// Fully self-contained: a minimal fake daemon (a unix socket the test owns)
/// reaches the trigger without a StatusNotifier host, and a **private**
/// `dbus-daemon` the test spawns supplies the bus — so this is a normal `#[test]`
/// that needs no ambient desktop and no external `dbus-run-session` wrapper.
#[tokio::test]
async fn engine_inserts_history_text_on_daemon_request() {
    const ENTRY: &str = "Deploy traefik and nginx";

    let Some(bus) = PrivateBus::start() else {
        // dbus-daemon isn't installed; nothing we can do but say so loudly.
        // (CI installs the `dbus` package, so this path is dev-machine only.)
        panic!("dbus-daemon not found — install the 'dbus' package to run engine e2e tests");
    };

    let fixture = Fixture::new("e2e-insert");
    // Bind the daemon socket BEFORE the engine starts so its connect succeeds.
    let listener = UnixListener::bind(fixture.socket_path()).expect("bind daemon socket");
    let engine = fixture.spawn_engine_on_bus(bus.address());

    // Accept the engine's connection and complete the handshake (it blocks on the
    // ServerHello before it will claim the bus name).
    let (stream, _) = listener.accept().expect("engine connects");
    let mut server_writer = stream.try_clone().expect("clone");
    let mut server_reader = BufReader::new(stream);
    let mut hello = String::new();
    server_reader
        .read_line(&mut hello)
        .expect("read ClientHello");
    assert!(
        matches!(decode_json_line(&hello), Ok(IpcMessage::ClientHello(_))),
        "engine should greet with ClientHello"
    );
    send_line(
        &mut server_writer,
        &IpcMessage::ServerHello(ServerHello {
            protocol_version: PROTOCOL_VERSION,
            accepted_features: vec![],
        }),
    );

    // Connect to the test's own private bus explicitly (no ambient session bus).
    let conn = zbus::connection::Builder::address(bus.address())
        .expect("valid bus address")
        .build()
        .await
        .expect("connect to private bus");
    let factory = await_factory(&conn).await;
    let engine_path: OwnedObjectPath = factory
        .call("CreateEngine", &("idiolect",))
        .await
        .expect("CreateEngine");
    let engine_proxy = Proxy::new(&conn, BUS_NAME, &engine_path, ENGINE_IFACE)
        .await
        .expect("engine proxy");
    let mut commit_signals = engine_proxy
        .receive_signal("CommitText")
        .await
        .expect("subscribe CommitText");

    // Focus the context so the engine knows where to commit.
    engine_proxy
        .call::<_, _, ()>("FocusIn", &())
        .await
        .expect("FocusIn");

    // The daemon pushes the stored entry; the engine commits it into the app.
    send_line(
        &mut server_writer,
        &IpcMessage::InsertText(InsertText {
            text: ENTRY.to_owned(),
        }),
    );

    let committed = {
        let msg = next_signal(&mut commit_signals).await;
        let body = msg.body();
        let (text,): (Value<'_>,) = body.deserialize().expect("commit body");
        extract_ibus_text(&text)
    };
    assert_eq!(
        committed, ENTRY,
        "history entry is typed at the cursor via CommitText"
    );

    drop(engine);
    drop(conn);
}

fn send_line(writer: &mut impl Write, message: &IpcMessage) {
    writer
        .write_all(encode_json_line(message).expect("encode").as_bytes())
        .expect("write message");
    writer.flush().expect("flush");
}

async fn await_factory(conn: &zbus::Connection) -> Proxy<'static> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await.expect("dbus proxy");
    let name = zbus::names::BusName::try_from(BUS_NAME).expect("valid bus name");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !dbus.name_has_owner(name.clone()).await.unwrap_or(false) {
        if Instant::now() >= deadline {
            panic!("engine did not claim {BUS_NAME} on the bus");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Proxy::new(conn, BUS_NAME, FACTORY_PATH, FACTORY_IFACE)
        .await
        .expect("factory proxy")
}

async fn process_key(engine: &Proxy<'_>, keyval: u32, state: u32) {
    let args = (keyval, 0u32, state);
    let call = engine.call::<_, _, bool>("ProcessKeyEvent", &args);
    tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .expect("ProcessKeyEvent should not hang")
        .expect("ProcessKeyEvent");
}

async fn next_signal<S>(stream: &mut S) -> zbus::Message
where
    S: futures_util::Stream<Item = zbus::Message> + Unpin,
{
    use futures_util::StreamExt;
    tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("signal within timeout")
        .expect("signal stream open")
}

/// Pull the text field (index 2) out of an `IBusText` structure value.
fn extract_ibus_text(value: &Value<'_>) -> String {
    let structure = match value {
        Value::Structure(s) => s,
        other => panic!("expected IBusText structure, got {other:?}"),
    };
    match &structure.fields()[2] {
        Value::Str(s) => s.as_str().to_owned(),
        other => panic!("expected text field, got {other:?}"),
    }
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
        let root = std::env::temp_dir().join(format!(
            "idiolect-ibus-e2e-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(root.join("runtime")).expect("runtime dir");
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
                .map_err(|e| e.to_string())
        })
    }

    fn spawn_engine(&self) -> EngineProc {
        // Engine resolves its socket as $XDG_RUNTIME_DIR/idiolect.sock. Force it
        // onto the test's session bus via IBUS_ADDRESS (otherwise it would read
        // the real ibus address file and join the wrong bus).
        let session_bus = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .expect("DBUS_SESSION_BUS_ADDRESS set in the test session");
        self.spawn_engine_on_bus(&session_bus)
    }

    /// Spawn the engine joined to a specific bus address (used with a private bus
    /// the test owns, so no ambient session bus is required).
    fn spawn_engine_on_bus(&self, ibus_address: &str) -> EngineProc {
        let child = Command::new(env!("CARGO_BIN_EXE_ibus-engine-idiolect"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("IBUS_ADDRESS", ibus_address)
            .spawn()
            .expect("engine binary should spawn");
        EngineProc(child)
    }

    fn write_config(&self) {
        fs::create_dir_all(self.model_path().parent().unwrap()).unwrap();
        fs::write(self.model_path(), b"dummy model").unwrap();
        fs::write(
            self.config_path(),
            format!(
                r#"[user]
default_user_id = "default"
[daemon]
socket_path = "{socket}"
log_level = "info"
[audio]
# Live-lifecycle fixture: holds the "mic" open between toggles so the daemon
# pushes recording=true, then yields the deterministic clip on stop. The plain
# "fixture" device transcribes instantly without ever announcing recording —
# the engine (correctly) ignores such an unannounced transcript.
input_device = "fixture-live"
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
private_text_probe = "x"
[observability]
log_private_text = false
"#,
                socket = self.socket_path().display(),
                data = self.data_dir().display(),
                db = self.database_path().display(),
            ),
        )
        .unwrap();
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

fn wait_for_socket(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        if Instant::now() >= deadline {
            panic!("daemon socket {} never appeared", path.display());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn assert_daemon_ok(daemon: JoinHandle<Result<(), String>>) {
    match daemon.join() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("daemon error: {e}"),
        Err(_) => panic!("daemon panicked"),
    }
}

fn open_store(path: &Path) -> SqliteMetadataStore {
    let mut store = SqliteMetadataStore::open_path(path).expect("db open");
    store.migrate().expect("migrate");
    store
}

/// Owns the engine child process and kills it on drop (so a test panic never
/// leaves a stray engine holding the bus name).
struct EngineProc(Child);

impl Drop for EngineProc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A private `dbus-daemon` session bus the test owns end-to-end: it makes the e2e
/// self-contained (no ambient desktop bus, no external `dbus-run-session`), and is
/// killed on drop. `--print-address --nofork` prints the bus address on its first
/// stdout line and then runs in the foreground until we kill it.
struct PrivateBus {
    child: Child,
    address: String,
}

impl PrivateBus {
    fn start() -> Option<Self> {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--print-address", "--nofork"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut address = String::new();
        BufReader::new(child.stdout.take().expect("piped stdout"))
            .read_line(&mut address)
            .expect("read bus address");
        let address = address.trim().to_owned();
        assert!(
            address.starts_with("unix:"),
            "unexpected bus address {address:?}"
        );
        Some(Self { child, address })
    }

    fn address(&self) -> &str {
        &self.address
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
