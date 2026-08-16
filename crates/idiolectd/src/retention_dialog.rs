//! Custom training-retention input, abstracted behind a trait so the GUI toolkit
//! is swappable and the daemon's logic stays testable. The default implementation
//! launches the `idiolect-retention-dialog` binary out-of-process and reads the
//! chosen day-count from its stdout (see that crate for the wire contract).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use idiolect_process::dialog::{CANCELLED_MARKER, EXIT_CANCELLED};
use idiolect_process::{ExpectedExit, FailureReporter, ObservedChild};

use crate::DAEMON_UNIT;

/// Prompts the user for a custom retention window, in days. Returns `None` if the
/// user cancels or the dialog can't run — the caller then leaves the setting as-is.
pub(crate) trait RetentionDialog: Send + Sync {
    fn prompt_days(&self, current_days: u32) -> Option<u32>;
}

/// Launches an external dialog binary, passing the current value as an argument
/// and reading the chosen day-count back from stdout. Keeping it out-of-process
/// means the dialog's GUI stack never runs inside the daemon.
pub(crate) struct SubprocessRetentionDialog {
    binary: PathBuf,
    reporter: FailureReporter,
}

impl SubprocessRetentionDialog {
    #[cfg(test)]
    pub(crate) fn new(binary: impl Into<PathBuf>) -> Self {
        Self::with_notifier(binary, String::new())
    }

    pub(crate) fn with_notifier(
        binary: impl Into<PathBuf>,
        notify_command: impl Into<String>,
    ) -> Self {
        Self {
            binary: binary.into(),
            reporter: FailureReporter::new(notify_command).with_journal_unit(DAEMON_UNIT),
        }
    }

    /// The notify command this launcher reports failures through.
    #[cfg(test)]
    pub(crate) fn notify_command(&self) -> &str {
        self.reporter.notify_command()
    }

    /// Find the dialog binary next to the running daemon binary, else by name.
    pub(crate) fn discover(notify_command: &str) -> Self {
        const NAME: &str = "idiolect-retention-dialog";
        let beside_daemon = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)))
            .filter(|path| path.exists());
        Self::with_notifier(
            beside_daemon.unwrap_or_else(|| PathBuf::from(NAME)),
            notify_command,
        )
    }
}

impl RetentionDialog for SubprocessRetentionDialog {
    fn prompt_days(&self, current_days: u32) -> Option<u32> {
        let mut command = Command::new(&self.binary);
        command
            .arg(current_days.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        let mut child =
            ObservedChild::spawn(&mut command, "Retention dialog", self.reporter.clone())?;
        let mut stdout = Vec::new();
        let read_error = match child.child_mut().stdout.take() {
            Some(mut pipe) => pipe
                .read_to_end(&mut stdout)
                .err()
                .map(|error| format!("could not read dialog output: {error}")),
            None => Some("could not read dialog output: stdout pipe unavailable".to_owned()),
        };
        let text = String::from_utf8_lossy(&stdout).into_owned();
        // Exit 1 counts as a cancel ONLY when the dialog said so on its way
        // out. libX11's I/O-error handler exits 1 from underneath `main`, so
        // without the marker an exit of 1 means the dialog died instead.
        let expected = if text == CANCELLED_MARKER {
            ExpectedExit::shares_our_lifecycle(&[0, EXIT_CANCELLED])
        } else {
            ExpectedExit::shares_our_lifecycle(&[0])
        };
        let status = child.wait_with_diagnostic(expected, read_error.as_deref())?;
        if read_error.is_some() || status.code() != Some(0) {
            return None;
        }
        text.trim().parse::<u32>().ok().filter(|days| *days >= 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idiolect_test_support::notifications::{write_executable_script, NotificationRecorder};
    use std::time::Duration;

    #[test]
    fn reads_a_day_count_from_a_successful_dialog() {
        // `echo 540` stands in for the dialog confirming a custom value.
        let dialog = SubprocessRetentionDialog::new("/bin/echo");
        // /bin/echo ignores the current-value arg and just prints "540".
        let dialog = WithArgs::new(dialog, "540");
        assert_eq!(dialog.prompt_days(365), Some(540));
    }

    #[test]
    fn a_dialog_that_produces_no_value_leaves_the_setting_alone() {
        // `false` exits 1 with no marker — a death, not a cancel — but either
        // way the caller must not change the retention window.
        let dialog = SubprocessRetentionDialog::new("false");
        assert_eq!(dialog.prompt_days(365), None);
    }

    /// Writes a stand-in dialog. `write_executable_script` blocks until the file
    /// can actually be exec'd — writing and exec'ing it immediately races with
    /// sibling test threads forking, which fails with `ETXTBSY`.
    fn stub_dialog(dir: &tempfile::TempDir, script: &str) -> PathBuf {
        let path = dir.path().join("dialog");
        write_executable_script(&path, script);
        path
    }

    #[test]
    fn cancelling_does_not_alert_even_when_the_gl_stack_printed_a_warning() {
        // A cancel announces itself with the marker; a healthy GL stack still
        // writes driver noise to stderr on this very machine. Treating that
        // pair as a crash alerted the user on every cancel.
        let recorder = NotificationRecorder::new();
        let dir = tempfile::tempdir().expect("temporary dialog directory");
        let dialog = stub_dialog(
            &dir,
            "#!/bin/sh\nprintf 'glx: failed to create dri3 screen\\n' >&2\nprintf '\\004cancelled'\nexit 1\n",
        );
        let dialog =
            SubprocessRetentionDialog::with_notifier(&dialog, recorder.command().to_owned());

        assert_eq!(dialog.prompt_days(365), None);

        std::thread::sleep(Duration::from_millis(100));
        // The recorder proved it can record when it was constructed, so an
        // empty log here means nothing was notified — not that it is broken.
        assert!(
            recorder.records().is_empty(),
            "user cancellation emitted a crash alert: {:?}",
            recorder.records()
        );
    }

    #[test]
    fn dying_with_the_cancel_code_but_no_marker_alerts_the_user() {
        // libX11's I/O-error handler exits 1 from underneath `main`, so the
        // dialog never reaches its cancel path. Judged on the code alone this
        // looks exactly like Cancel and the failure disappears.
        let recorder = NotificationRecorder::new();
        let dir = tempfile::tempdir().expect("temporary dialog directory");
        let dialog = stub_dialog(
            &dir,
            "#!/bin/sh\nprintf 'X connection to :0 broken\\n' >&2\nexit 1\n",
        );
        let dialog =
            SubprocessRetentionDialog::with_notifier(&dialog, recorder.command().to_owned());

        assert_eq!(dialog.prompt_days(365), None);

        let alert = recorder.wait();
        assert!(alert.contains("status 1"), "{alert}");
        assert!(alert.contains("X connection"), "{alert}");
    }

    #[test]
    fn a_dialog_that_cannot_start_alerts_the_user_with_its_reason() {
        let recorder = NotificationRecorder::new();
        let dir = tempfile::tempdir().expect("temporary dialog directory");
        let dialog = stub_dialog(
            &dir,
            "#!/bin/sh\nprintf 'Glutin BadAttribute\\n' >&2\nexit 2\n",
        );
        let dialog =
            SubprocessRetentionDialog::with_notifier(&dialog, recorder.command().to_owned());

        assert_eq!(dialog.prompt_days(365), None);

        let alert = recorder.wait();
        assert!(alert.contains("status 2"), "{alert}");
        assert!(alert.contains("Glutin BadAttribute"), "{alert}");
    }

    #[test]
    fn rejects_non_numeric_or_zero_output() {
        let dialog = WithArgs::new(SubprocessRetentionDialog::new("/bin/echo"), "0");
        assert_eq!(dialog.prompt_days(365), None);
        let dialog = WithArgs::new(SubprocessRetentionDialog::new("/bin/echo"), "abc");
        assert_eq!(dialog.prompt_days(365), None);
    }

    /// Test shim: `/bin/echo` prints whatever fixed args we give it (ignoring the
    /// current-value arg the real launcher appends), so we can stand in for the
    /// dialog's stdout without a GUI.
    struct WithArgs {
        binary: PathBuf,
        output: String,
    }

    impl WithArgs {
        fn new(inner: SubprocessRetentionDialog, output: &str) -> Self {
            Self {
                binary: inner.binary,
                output: output.to_owned(),
            }
        }
    }

    impl RetentionDialog for WithArgs {
        fn prompt_days(&self, _current_days: u32) -> Option<u32> {
            let out = Command::new(&self.binary).arg(&self.output).output().ok()?;
            if !out.status.success() {
                return None;
            }
            String::from_utf8(out.stdout)
                .ok()?
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|days| *days >= 1)
        }
    }
}
