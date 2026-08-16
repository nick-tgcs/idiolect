//! Out-of-process Settings window. DBusMenu menus close on every click and the
//! protocol cannot keep them open, so multi-choice settings live in a real
//! window that stays open while the user adjusts several things.
//!
//! Subprocess contract (the daemon side of `idiolect-settings`):
//!   stdin  : ONE line — a JSON object with the current effective settings.
//!   stdout : one tray action id per line (e.g. `settings:pause:2`,
//!            `translation:output:41`, `review_mode`), emitted as the user
//!            changes things. The daemon applies each through the SAME path as
//!            a tray click, so the window and the menu can never disagree on
//!            semantics.
//!   exit   : whenever the user closes the window (click-off, Esc, X).
//!
//! Like every GUI here, it is best-effort: a missing or crashing binary must
//! never affect dictation.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use idiolect_adapter_ksni::TrayCallback;
use idiolect_process::{ExpectedExit, FailureReporter, ObservedChild};

use crate::DAEMON_UNIT;

pub(crate) struct SettingsLauncher {
    binary: PathBuf,
    args: Vec<String>,
    reporter: FailureReporter,
    /// True while a window is up — a second "Settings…" click is a no-op
    /// rather than a second window fighting over the same settings.
    window_open: Arc<AtomicBool>,
}

impl SettingsLauncher {
    #[cfg(test)]
    pub(crate) fn new(binary: impl Into<PathBuf>) -> Self {
        Self::with_command(binary, Vec::new())
    }

    /// Full constructor: binary + fixed arguments. Tests use `sh -c <script>`
    /// stand-ins so no temp script files are ever exec'd.
    #[cfg(test)]
    pub(crate) fn with_command(binary: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self::with_command_and_notifier(binary, args, String::new())
    }

    pub(crate) fn with_command_and_notifier(
        binary: impl Into<PathBuf>,
        args: Vec<String>,
        notify_command: impl Into<String>,
    ) -> Self {
        Self {
            binary: binary.into(),
            args,
            reporter: FailureReporter::new(notify_command).with_journal_unit(DAEMON_UNIT),
            window_open: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The notify command this launcher reports failures through.
    #[cfg(test)]
    pub(crate) fn notify_command(&self) -> &str {
        self.reporter.notify_command()
    }

    /// Find the settings binary next to the running daemon binary, falling
    /// back to its plain name (resolved via `PATH`).
    pub(crate) fn discover(notify_command: &str) -> Self {
        const NAME: &str = "idiolect-settings";
        let beside_daemon = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)))
            .filter(|path| path.exists());
        Self::with_command_and_notifier(
            beside_daemon.unwrap_or_else(|| PathBuf::from(NAME)),
            Vec::new(),
            notify_command,
        )
    }

    /// Open the window with the current state and forward every change it
    /// emits into the tray-callback channel (the run loop applies them on its
    /// next tick exactly like menu clicks). Returns immediately; the window is
    /// serviced by a dedicated thread that also reaps the process.
    pub(crate) fn open(&self, state_json: String, forward: mpsc::Sender<TrayCallback>) {
        if self.window_open.swap(true, Ordering::SeqCst) {
            return; // one window at a time
        }
        let mut command = Command::new(&self.binary);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let Some(mut child) = ObservedChild::spawn(&mut command, "Settings", self.reporter.clone())
        else {
            self.window_open.store(false, Ordering::SeqCst);
            return;
        };
        let stdin = child.child_mut().stdin.take();
        let stdout = child.child_mut().stdout.take();
        let window_open = Arc::clone(&self.window_open);
        std::thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = writeln!(stdin, "{state_json}");
                // Dropping stdin is fine: the window reads exactly one line.
            }
            if let Some(stdout) = stdout {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.is_empty() {
                        continue;
                    }
                    if forward.send(TrayCallback::Activate(line)).is_err() {
                        break; // run loop gone; stop forwarding
                    }
                }
            }
            let _ = child.wait(ExpectedExit::shares_our_lifecycle(&[0]));
            window_open.store(false, Ordering::SeqCst);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idiolect_test_support::notifications::NotificationRecorder;
    use std::time::{Duration, Instant};

    fn script_launcher(script: &str) -> SettingsLauncher {
        SettingsLauncher::with_command("sh", vec!["-c".to_owned(), script.to_owned()])
    }

    #[test]
    fn forwards_window_changes_as_tray_activations_and_delivers_state() {
        // The stand-in window echoes one change, then proves it received the
        // state line.
        let launcher = script_launcher(
            r#"IFS= read -r state
               printf 'settings:pause:2\n'
               printf 'state:%s\n' "$state""#,
        );
        let (tx, rx) = mpsc::channel();
        launcher.open(r#"{"pause_ms":700}"#.to_owned(), tx);

        let TrayCallback::Activate(first) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first forwarded action");
        assert_eq!(
            first, "settings:pause:2",
            "stdout lines become tray actions"
        );
        let TrayCallback::Activate(second) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("second forwarded action");
        assert_eq!(
            second, r#"state:{"pause_ms":700}"#,
            "the window received the state JSON on stdin"
        );
    }

    #[test]
    fn only_one_window_at_a_time_and_reopens_after_exit() {
        // Each spawn announces itself; a second open while the first window is
        // up must NOT spawn again, but after it exits a reopen must.
        let launcher = script_launcher(
            r#"printf 'spawned\n'
               sleep 0.4"#,
        );
        let (tx, rx) = mpsc::channel();
        launcher.open(String::new(), tx.clone());
        launcher.open(String::new(), tx.clone()); // ignored: still open
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "first spawn"
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(700)).is_err(),
            "no second window while the first is open"
        );
        // The first window has exited by now (sleep 0.4 + margin elapsed).
        launcher.open(String::new(), tx);
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "reopens once the previous window closed"
        );
    }

    #[test]
    fn a_missing_binary_is_a_safe_noop_that_does_not_wedge_the_launcher() {
        let launcher = SettingsLauncher::new("/nonexistent/idiolect-settings-xyz");
        let (tx, rx) = mpsc::channel();
        launcher.open(String::new(), tx.clone()); // must not panic
        launcher.open(String::new(), tx); // and must not be wedged "open"
        assert!(rx.try_recv().is_err(), "nothing forwarded");
        assert!(
            !launcher.window_open.load(Ordering::SeqCst),
            "failed spawn releases the open flag"
        );
    }

    #[test]
    fn a_crashing_window_alerts_the_user_with_its_exit_and_stderr() {
        let recorder = NotificationRecorder::new();
        let launcher = SettingsLauncher::with_command_and_notifier(
            "sh",
            vec![
                "-c".to_owned(),
                "printf 'Glutin BadAttribute\\n' >&2; exit 23".to_owned(),
            ],
            recorder.command().to_owned(),
        );
        let (tx, _rx) = mpsc::channel();

        launcher.open(String::new(), tx);

        let alert = recorder.wait();
        assert!(alert.contains("Idiolect Settings failed"), "{alert}");
        assert!(alert.contains("status 23"), "{alert}");
        assert!(alert.contains("Glutin BadAttribute"), "{alert}");
        assert!(alert.contains("journalctl --user -u idiolectd"), "{alert}");
    }

    #[test]
    fn a_spawn_failure_alerts_the_user_with_the_os_error() {
        let recorder = NotificationRecorder::new();
        let launcher = SettingsLauncher::with_command_and_notifier(
            "/nonexistent/idiolect-settings-xyz",
            Vec::new(),
            recorder.command().to_owned(),
        );
        let (tx, _rx) = mpsc::channel();

        launcher.open(String::new(), tx);

        let alert = recorder.wait();
        assert!(alert.contains("Idiolect Settings failed"), "{alert}");
        assert!(alert.contains("could not start"), "{alert}");
        assert!(alert.contains("No such file or directory"), "{alert}");
    }

    #[test]
    fn a_normal_window_close_does_not_alert() {
        let recorder = NotificationRecorder::new();
        let launcher = SettingsLauncher::with_command_and_notifier(
            "sh",
            vec!["-c".to_owned(), "exit 0".to_owned()],
            recorder.command().to_owned(),
        );
        let (tx, _rx) = mpsc::channel();

        launcher.open(String::new(), tx);

        let deadline = Instant::now() + Duration::from_secs(5);
        while launcher.window_open.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "window did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(!recorder.log_path().exists(), "clean exit emitted an alert");
    }
}
