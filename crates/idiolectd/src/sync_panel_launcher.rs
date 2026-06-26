//! Out-of-process Corrections Dashboard. The dashboard shows phone-sync status,
//! pairing QR, and training controls.
//!
//! Subprocess contract (the daemon side of `idiolect-app`):
//!   args   : `--standalone` so the app owns its own sync server on first launch.
//!   stdout : one tray action id per line (`sync:pair`, `train:now`, …).
//!   exit   : whenever the user closes the window.
//!
//! Like every GUI here, it is best-effort: a missing or crashing binary must
//! never affect dictation.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use idiolect_adapter_ksni::TrayCallback;

pub(crate) struct SyncPanelLauncher {
    binary: PathBuf,
    args: Vec<String>,
    window_open: Arc<AtomicBool>,
}

impl SyncPanelLauncher {
    pub(crate) fn new(binary: impl Into<PathBuf>) -> Self {
        Self::with_command(binary, Vec::new())
    }

    pub(crate) fn with_command(binary: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            binary: binary.into(),
            args,
            window_open: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Find the dashboard binary next to the running daemon binary, falling
    /// back to its plain name (resolved via `PATH`).
    pub(crate) fn discover() -> Self {
        const NAME: &str = "idiolect-app";
        let beside_daemon = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)))
            .filter(|path| path.exists());
        Self::new(beside_daemon.unwrap_or_else(|| PathBuf::from(NAME)))
    }

    /// Open the dashboard. Returns immediately; the window is serviced by a
    /// dedicated thread that also reaps the process. A second call while the
    /// dashboard is open is a no-op.
    pub(crate) fn open(&self, forward: mpsc::Sender<TrayCallback>) {
        if self.window_open.swap(true, Ordering::SeqCst) {
            return;
        }
        let spawned = Command::new(&self.binary)
            .args(&self.args)
            .arg("--standalone")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = spawned else {
            self.window_open.store(false, Ordering::SeqCst);
            return;
        };
        let stdout = child.stdout.take();
        let window_open = Arc::clone(&self.window_open);
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                for line in std::io::BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.is_empty() {
                        continue;
                    }
                    if forward.send(TrayCallback::Activate(line)).is_err() {
                        break;
                    }
                }
            }
            let _ = child.wait();
            window_open.store(false, Ordering::SeqCst);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn script_launcher(script: &str) -> SyncPanelLauncher {
        SyncPanelLauncher::with_command("sh", vec!["-c".to_owned(), script.to_owned()])
    }

    #[test]
    fn forwards_dashboard_actions_as_tray_activations() {
        let launcher = script_launcher(r#"printf 'train:now\nsync:pair\n'"#);
        let (tx, rx) = mpsc::channel();
        launcher.open(tx);

        let TrayCallback::Activate(first) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first action");
        assert_eq!(first, "train:now");
        let TrayCallback::Activate(second) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("second action");
        assert_eq!(second, "sync:pair");
    }

    #[test]
    fn only_one_window_at_a_time_and_reopens_after_exit() {
        let launcher = script_launcher(
            r#"printf 'spawned\n'
               sleep 0.4"#,
        );
        let (tx, rx) = mpsc::channel();
        launcher.open(tx.clone());
        launcher.open(tx.clone()); // ignored: still open
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "first spawn"
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(700)).is_err(),
            "no second window while the first is open"
        );
        launcher.open(tx);
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "reopens once the previous window closed"
        );
    }

    #[test]
    fn a_missing_binary_is_a_safe_noop_that_does_not_wedge_the_launcher() {
        let launcher = SyncPanelLauncher::new("/nonexistent/idiolect-app-xyz");
        let (tx, rx) = mpsc::channel();
        launcher.open(tx.clone());
        launcher.open(tx);
        assert!(rx.try_recv().is_err(), "nothing forwarded");
        assert!(
            !launcher.window_open.load(Ordering::SeqCst),
            "failed spawn releases the open flag"
        );
    }
}
