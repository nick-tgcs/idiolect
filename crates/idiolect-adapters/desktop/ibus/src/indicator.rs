//! "Voice is live" indicator abstraction. While dictation is recording the
//! engine shows a small mic overlay next to the caret; this hides the concrete
//! GUI behind a trait (and, by default, a process boundary) so it is swappable
//! and its dependencies stay out of the IME.

use std::io::Write;
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Mutex;

use idiolect_process::{FailureReporter, ObservedChild};

/// Shows the recording indicator at a caret position, repositions it while it's
/// already showing, and hides it. All calls are idempotent.
pub trait RecordingIndicator: Send + Sync {
    /// Show at, or move to, the given caret screen position.
    fn show(&self, x: i32, y: i32);
    fn hide(&self);
}

struct Running {
    child: ObservedChild,
    stdin: ChildStdin,
}

/// Launches an external overlay binary, streaming caret positions to its stdin
/// so it tracks the text caret, and kills it to hide. Keeping it out-of-process
/// means the overlay's GUI stack never runs inside the async IME.
pub struct SubprocessIndicator {
    binary: PathBuf,
    reporter: FailureReporter,
    state: Mutex<Option<Running>>,
}

impl SubprocessIndicator {
    #[cfg(test)]
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self::with_notifier(binary, String::new())
    }

    /// As [`Self::new`], plus the command used to tell the user when the
    /// overlay fails.
    pub fn with_notifier(binary: impl Into<PathBuf>, notify_command: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            // The overlay is cosmetic and `show` runs on every caret update,
            // so repeats are suppressed; its diagnostics still need a file,
            // because the engine's stderr is discarded.
            reporter: FailureReporter::new(notify_command)
                .with_log_file(crate::notify::diagnostics_log_path()),
            state: Mutex::new(None),
        }
    }

    /// The notify command this overlay reports failures through.
    #[cfg(test)]
    pub(crate) fn notify_command(&self) -> &str {
        self.reporter.notify_command()
    }

    /// Find the overlay binary next to the running engine binary, else by name.
    pub fn discover(notify_command: &str) -> Self {
        const NAME: &str = "idiolect-recording-indicator";
        let beside_engine = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)))
            .filter(|path| path.exists());
        Self::with_notifier(
            beside_engine.unwrap_or_else(|| PathBuf::from(NAME)),
            notify_command,
        )
    }
}

impl RecordingIndicator for SubprocessIndicator {
    fn show(&self, x: i32, y: i32) {
        let mut guard = self.state.lock().expect("indicator mutex");
        if let Some(running) = guard.as_mut() {
            // Already showing — stream the new caret position so it follows.
            let _ = writeln!(running.stdin, "{x} {y}");
            let _ = running.stdin.flush();
            return;
        }
        let mut command = Command::new(&self.binary);
        command
            .arg(x.to_string())
            .arg(y.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null());
        if let Some(mut child) =
            ObservedChild::spawn(&mut command, "Recording indicator", self.reporter.clone())
        {
            match child.child_mut().stdin.take() {
                Some(stdin) => *guard = Some(Running { child, stdin }),
                // No stdin means no protocol; we are closing it on purpose.
                None => child.dismiss(),
            }
        }
    }

    fn hide(&self) {
        if let Some(running) = self.state.lock().expect("indicator mutex").take() {
            running.child.dismiss();
        }
    }
}

impl Drop for SubprocessIndicator {
    fn drop(&mut self) {
        // The engine going away takes any visible overlay with it. That is us
        // hiding the overlay, not the overlay failing — alerting on shutdown
        // would be pure noise. `lock()` is tolerated failing here: a poisoned
        // mutex during teardown must not turn into a panic in a `Drop`.
        if let Ok(mut guard) = self.state.lock() {
            if let Some(running) = guard.take() {
                running.child.dismiss();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tearing_the_engine_down_while_showing_does_not_alert() {
        let recorder = idiolect_test_support::notifications::NotificationRecorder::new();
        let indicator = SubprocessIndicator::with_notifier("cat", recorder.command().to_owned());
        indicator.show(30, 30);
        assert!(indicator.state.lock().unwrap().is_some(), "spawned");

        let started = std::time::Instant::now();
        drop(indicator);

        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "teardown waited for the overlay instead of closing it: {:?}",
            started.elapsed()
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            recorder.records().is_empty(),
            "engine teardown alerted the user: {:?}",
            recorder.records()
        );
    }

    #[test]
    fn show_then_hide_a_short_lived_process() {
        // `cat` stands in for the overlay: it reads stdin (our position stream)
        // and stays alive until hide() kills it.
        let indicator = SubprocessIndicator::new("cat");
        indicator.show(30, 30);
        assert!(indicator.state.lock().unwrap().is_some(), "spawned");
        // Showing again repositions via stdin rather than respawning.
        indicator.show(40, 50);
        assert!(
            indicator.state.lock().unwrap().is_some(),
            "still one process"
        );
        indicator.hide();
        assert!(indicator.state.lock().unwrap().is_none(), "killed");
        indicator.hide(); // no-op
    }
}
