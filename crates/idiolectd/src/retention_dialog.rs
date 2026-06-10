//! Custom training-retention input, abstracted behind a trait so the GUI toolkit
//! is swappable and the daemon's logic stays testable. The default implementation
//! launches the `idiolect-retention-dialog` binary out-of-process and reads the
//! chosen day-count from its stdout (see that crate for the wire contract).

use std::path::PathBuf;
use std::process::Command;

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
}

impl SubprocessRetentionDialog {
    pub(crate) fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    /// Find the dialog binary next to the running daemon binary, else by name.
    pub(crate) fn discover() -> Self {
        const NAME: &str = "idiolect-retention-dialog";
        let beside_daemon = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)))
            .filter(|path| path.exists());
        Self::new(beside_daemon.unwrap_or_else(|| PathBuf::from(NAME)))
    }
}

impl RetentionDialog for SubprocessRetentionDialog {
    fn prompt_days(&self, current_days: u32) -> Option<u32> {
        let output = Command::new(&self.binary)
            .arg(current_days.to_string())
            .output()
            .ok()?;
        if !output.status.success() {
            return None; // cancelled, or the dialog failed to launch
        }
        String::from_utf8(output.stdout)
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
