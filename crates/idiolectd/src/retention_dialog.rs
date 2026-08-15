//! Custom training-retention input, abstracted behind a trait so the GUI toolkit
//! is swappable and the daemon's logic stays testable. The default implementation
//! launches the `idiolect-retention-dialog` binary out-of-process and reads the
//! chosen day-count from its stdout (see that crate for the wire contract).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::observed_child::{FailureReporter, ObservedChild};

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
            reporter: FailureReporter::new(notify_command),
        }
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
        let status = child.wait_with_diagnostic(&[0, 1], read_error.as_deref())?;
        if read_error.is_some() || status.code() != Some(0) {
            return None; // exit 1 is the dialog's documented user-cancel code
        }
        String::from_utf8(stdout)
            .ok()?
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|days| *days >= 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::NotificationRecorder;
    use std::os::unix::fs::PermissionsExt;
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
    fn a_failed_or_cancelled_dialog_yields_none() {
        // `false` exits non-zero -> treated as a cancel.
        let dialog = SubprocessRetentionDialog::new("false");
        assert_eq!(dialog.prompt_days(365), None);
    }

    #[test]
    fn documented_cancel_exit_does_not_alert_the_user() {
        let recorder = NotificationRecorder::new();
        let dialog =
            SubprocessRetentionDialog::with_notifier("false", recorder.command().to_owned());

        assert_eq!(dialog.prompt_days(365), None);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !recorder.log_path().exists(),
            "user cancellation emitted a crash alert"
        );
    }

    #[test]
    fn cancel_code_with_stderr_is_treated_as_a_crash() {
        let recorder = NotificationRecorder::new();
        let dir = tempfile::tempdir().expect("temporary dialog directory");
        let crashing_dialog = dir.path().join("dialog");
        std::fs::write(
            &crashing_dialog,
            "#!/bin/sh\nprintf 'Glutin BadAttribute\\n' >&2\nexit 1\n",
        )
        .expect("write crashing dialog");
        std::fs::set_permissions(&crashing_dialog, std::fs::Permissions::from_mode(0o755))
            .expect("chmod test executable");
        let dialog = SubprocessRetentionDialog::with_notifier(
            &crashing_dialog,
            recorder.command().to_owned(),
        );

        assert_eq!(dialog.prompt_days(365), None);
        let alert = recorder.wait();
        assert!(alert.contains("status 1"), "{alert}");
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
