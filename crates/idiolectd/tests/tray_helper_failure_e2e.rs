//! End-to-end: a real tray activation, over a real (private) D-Bus session,
//! reaching a real helper process, whose failure reaches the user's configured
//! notifier.
//!
//! Every other level stops short of this. The unit tests build a launcher
//! directly; the integration tests drive `ObservedChild` with `sh` stand-ins;
//! `every_helper_launcher_reports_through_the_configured_notify_command` pins
//! the config-to-launcher wiring but calls nothing. Nothing proved that a menu
//! click actually arrives at a launcher and comes back out as a notification.
//!
//! Two things make this reachable without a desktop, and both are already
//! relied on elsewhere in the repo: `KsniTray` calls `assume_sni_available`, so
//! the `com.canonical.dbusmenu` object is exported even with no
//! StatusNotifierWatcher (see `tray_reconnect.rs`), and the daemon resolves its
//! helpers beside its OWN binary — so a copy of the daemon in a temp directory
//! picks up the stub helper next to it.
//!
//! Skips, rather than fails, when `dbus-daemon`/`busctl` are absent — the same
//! convention `tray_reconnect.rs` uses.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{ClientHello, IpcMessage, FEATURE_COMMIT, FEATURE_PREEDIT};

/// Generous: this boots a real daemon, a real bus and a real subprocess.
const BUDGET: Duration = Duration::from_secs(30);

#[test]
fn a_tray_click_on_a_broken_helper_reaches_the_users_notifier() {
    let Some(bus) = PrivateBus::start() else {
        println!("dbus-daemon not found — install the 'dbus' package to run this test");
        return;
    };
    if which("busctl").is_none() {
        println!("busctl not found — install systemd's client tools to run this test");
        return;
    }

    let fixture = Fixture::new();
    let mut daemon = fixture.spawn_daemon(&bus.address);

    // The daemon services tray callbacks inside its connection loop, so a
    // client has to be attached before a click is looked at.
    let mut stream = fixture.connect_client();
    let mut reader = BufReader::new(stream.try_clone().expect("clone client stream"));
    send_hello(&mut stream, &mut reader);

    let service = bus
        .await_status_notifier_item()
        .expect("the daemon should export a StatusNotifierItem on the private bus");
    let item = bus
        .menu_item_id(&service, "Settings")
        .expect("the tray menu should carry a Settings entry");
    bus.click(&service, item);

    let alert = fixture.await_notification();

    let _ = daemon.kill();
    let _ = daemon.wait();

    assert!(
        alert.contains("Idiolect Settings failed"),
        "the click never came back as a failure: {alert}"
    );
    assert!(alert.contains("status 23"), "{alert}");
    assert!(alert.contains("no GPU adapter"), "{alert}");
}

fn which(binary: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(binary))
            .find(|candidate| candidate.exists())
    })
}

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
        Some(Self {
            child,
            address: address.trim().to_owned(),
        })
    }

    fn busctl(&self, args: &[&str]) -> String {
        let output = Command::new("busctl")
            .arg(format!("--address={}", self.address))
            .args(args)
            .output()
            .expect("run busctl");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn await_status_notifier_item(&self) -> Option<String> {
        let deadline = Instant::now() + BUDGET;
        while Instant::now() < deadline {
            let listing = self.busctl(&["list"]);
            if let Some(service) = listing
                .split_whitespace()
                .find(|name| name.starts_with("org.kde.StatusNotifierItem-"))
            {
                return Some(service.to_owned());
            }
            thread::sleep(Duration::from_millis(100));
        }
        None
    }

    /// The DBusMenu id of the item whose label starts with `label`.
    ///
    /// Looked up rather than hardcoded: the ids are positional, so a hardcoded
    /// one silently starts pointing at a different menu entry the moment
    /// anything above it moves.
    fn menu_item_id(&self, service: &str, label: &str) -> Option<i32> {
        let layout = self.busctl(&[
            "call",
            service,
            "/MenuBar",
            "com.canonical.dbusmenu",
            "GetLayout",
            "iias",
            "--",
            "0",
            "-1",
            "1",
            "label",
        ]);
        let needle = format!("1 \"label\" s \"{label}");
        let entry = layout.find(&needle)?;
        // Walk back over ` <id> ` to the marker that precedes every item.
        let marker = "(ia{sv}av) ";
        let start = layout[..entry].rfind(marker)? + marker.len();
        layout[start..entry].trim().parse().ok()
    }

    fn click(&self, service: &str, item: i32) {
        self.busctl(&[
            "call",
            service,
            "/MenuBar",
            "com.canonical.dbusmenu",
            "Event",
            "isvu",
            "--",
            &item.to_string(),
            "clicked",
            "s",
            "",
            "0",
        ]);
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        // Kept short: the control socket lives under here and `sun_path` is
        // capped at 108 bytes.
        let root = std::env::temp_dir().join(format!(
            "idl-tray-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(root.join("install")).expect("install dir");
        fs::create_dir_all(root.join("data/models/whisper")).expect("model dir");
        fs::create_dir_all(root.join("runtime")).expect("runtime dir");
        let fixture = Self { root };
        fixture.write_model();
        fixture.write_helpers();
        fixture.write_config();
        fixture
    }

    fn write_model(&self) {
        fs::write(
            self.root.join("data/models/whisper/whisper-medium-en.bin"),
            b"dummy model",
        )
        .expect("dummy model");
    }

    /// A copy of the daemon, with a deliberately broken Settings window beside
    /// it — that is how `SettingsLauncher::discover` finds its helper.
    fn write_helpers(&self) {
        fs::copy(
            env!("CARGO_BIN_EXE_idiolectd"),
            self.root.join("install/idiolectd"),
        )
        .expect("copy the daemon beside its helpers");
        executable(
            &self.root.join("install/idiolect-settings"),
            "#!/bin/sh\nprintf 'no GPU adapter\\n' >&2\nexit 23\n",
        );
        executable(
            &self.notifier(),
            &format!(
                "#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"$2\" >> \"{}\"\n",
                self.notifications().display()
            ),
        );
    }

    fn write_config(&self) {
        fs::write(
            self.config_path(),
            format!(
                r#"[user]
default_user_id = "default"

[daemon]
socket_path = "{socket}"
log_level = "info"
notify_command = "{notifier}"

[storage]
data_dir = "{data}"
"#,
                socket = self.socket_path().display(),
                notifier = self.notifier().display(),
                data = self.root.join("data").display(),
            ),
        )
        .expect("config");
    }

    fn spawn_daemon(&self, bus_address: &str) -> Child {
        Command::new(self.root.join("install/idiolectd"))
            .args([
                "run",
                "--config",
                self.config_path().to_str().expect("utf8 config path"),
            ])
            .env("DBUS_SESSION_BUS_ADDRESS", bus_address)
            // The suite exports this to keep the tray out of every OTHER test;
            // this is the one test that needs the tray.
            .env_remove("IDIOLECT_DISABLE_TRAY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the daemon")
    }

    fn connect_client(&self) -> UnixStream {
        let deadline = Instant::now() + BUDGET;
        while Instant::now() < deadline {
            if let Ok(stream) = UnixStream::connect(self.socket_path()) {
                return stream;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("the daemon never opened {}", self.socket_path().display());
    }

    fn await_notification(&self) -> String {
        let deadline = Instant::now() + BUDGET;
        while Instant::now() < deadline {
            if let Ok(contents) = fs::read_to_string(self.notifications()) {
                if contents.ends_with('\n') {
                    return contents;
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("no notification arrived within {BUDGET:?}");
    }

    fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    fn socket_path(&self) -> PathBuf {
        self.root.join("runtime/d.sock")
    }

    fn notifier(&self) -> PathBuf {
        self.root.join("notify.sh")
    }

    fn notifications(&self) -> PathBuf {
        self.root.join("notifications.log")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write helper script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod helper script");
}

fn send_hello(stream: &mut UnixStream, reader: &mut BufReader<UnixStream>) {
    let hello = IpcMessage::ClientHello(ClientHello {
        client_name: "idiolectd-tray-e2e".to_owned(),
        protocol_version: 1,
        features: vec![FEATURE_PREEDIT.to_owned(), FEATURE_COMMIT.to_owned()],
    });
    let line = encode_json_line(&hello).expect("encode hello");
    stream.write_all(line.as_bytes()).expect("send hello");
    stream.flush().expect("flush hello");

    let mut response = String::new();
    reader.read_line(&mut response).expect("read server hello");
    match decode_json_line(&response).expect("decode server hello") {
        IpcMessage::ServerHello(_) => {}
        other => panic!("expected ServerHello, got {other:?}"),
    }
}
