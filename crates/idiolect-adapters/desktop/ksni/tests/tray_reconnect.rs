//! Integration test for tray reconnect after a boot-time race.
//!
//! # Three-level coverage
//!
//! - **Unit**: `is_live()` accessor in `src/lib.rs` — no D-Bus needed.
//! - **Integration** (this file): spins up a private `dbus-daemon` with *no*
//!   `StatusNotifierWatcher` and verifies the tray handle is live so the daemon
//!   can reconnect when GNOME Shell comes up.  Requires `dbus-daemon` in PATH.
//! - **End-to-end**: whether the icon physically appears in GNOME Shell's
//!   notification area is a GUI/desktop boundary with no headless seam — the
//!   integration level covers the D-Bus reconnect contract; visual rendering
//!   requires a real desktop shell.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use idiolect_adapter_ksni::KsniTray;

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
            "unexpected dbus-daemon address: {address:?}"
        );
        Some(Self { child, address })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Regression test for the boot-time tray race.
///
/// The daemon's systemd unit fires as soon as `graphical-session.target` is
/// reached, but GNOME Shell (which registers `org.kde.StatusNotifierWatcher`)
/// can take ~800 ms longer.  Without `assume_sni_available(true)`, ksni got
/// `ServiceUnknown`, returned `Err`, and the tray handle became `None` — no
/// reconnect, no icon, for the entire daemon session.
///
/// With the fix the ksni reconnect loop is started even when the watcher is
/// absent: `is_live()` is `true`, and the icon appears once GNOME Shell is up.
#[test]
fn tray_is_live_on_session_bus_with_no_status_notifier_watcher() {
    let Some(bus) = PrivateBus::start() else {
        println!("dbus-daemon not found — install the 'dbus' package to run this test");
        return;
    };

    // Point ksni at the watcher-free private bus for the duration of this test.
    // Safe here: this is the only test in this binary (separate integration-test
    // binary, not mixed with unit tests).
    let prev_bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
    let prev_disable = std::env::var("IDIOLECT_DISABLE_TRAY").ok();
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus.address);
    std::env::remove_var("IDIOLECT_DISABLE_TRAY");

    let (tx, _rx) = mpsc::channel();
    let result = KsniTray::new(tx);

    // Restore env regardless of what the assertion below does.
    match prev_bus {
        Some(addr) => std::env::set_var("DBUS_SESSION_BUS_ADDRESS", addr),
        None => std::env::remove_var("DBUS_SESSION_BUS_ADDRESS"),
    }
    if let Some(val) = prev_disable {
        std::env::set_var("IDIOLECT_DISABLE_TRAY", val);
    }

    let tray = result.expect("KsniTray::new must not return Err when the session bus is up");
    assert!(
        tray.is_live(),
        "tray must be live (reconnect loop running) when the session bus exists but \
         org.kde.StatusNotifierWatcher is not yet registered — simulates the boot-time \
         race between idiolectd.service and GNOME Shell"
    );
}
