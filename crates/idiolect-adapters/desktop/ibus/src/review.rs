//! Review-dialog abstraction. The engine shows the dictated text in a box it
//! controls, lets the user correct it, and gets the final text back — so the
//! correction is captured regardless of the destination application (it never
//! depends on the app reporting its contents).
//!
//! The concrete GUI is kept behind this trait and, by default, behind a process
//! boundary (`SubprocessReviewDialog`), so the toolkit is swappable with zero
//! impact on the engine and the GUI's heavy dependencies stay out of the IME.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::focus::{default_window_focus, NoopWindowFocus, WindowFocus};

/// After restoring focus, give the window manager + application a moment to
/// process the focus-in and re-establish their input context before the engine
/// commits — otherwise the commit can race the focus hand-back.
const FOCUS_SETTLE: Duration = Duration::from_millis(120);

/// Presents `transcript` for review and returns the user's final text, or
/// `None` if they cancelled. Implementations are toolkit-specific and fully
/// interchangeable.
pub trait ReviewDialog: Send + Sync {
    fn review(&self, transcript: &str) -> Option<String>;
}

/// Runs an external dialog binary: the transcript goes to its stdin, the edited
/// text comes back on stdout, and a non-zero exit means "cancelled". This both
/// hides the toolkit and keeps the GUI in its own process (so winit/egui never
/// run inside the async IME).
///
/// Because that dialog is a separate top-level window, it steals X11 focus from
/// the app the user was typing into; this type captures the active window before
/// showing it and restores focus afterwards (via [`WindowFocus`]) so the commit
/// lands in the right place and the user can immediately press Enter.
pub struct SubprocessReviewDialog {
    binary: PathBuf,
    focus: Box<dyn WindowFocus>,
}

impl SubprocessReviewDialog {
    /// Construct with no focus management (capture is a no-op). Used by tests.
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            focus: Box::new(NoopWindowFocus),
        }
    }

    /// Construct with an explicit focus manager (used to inject a fake in tests).
    pub fn with_focus(binary: impl Into<PathBuf>, focus: Box<dyn WindowFocus>) -> Self {
        Self {
            binary: binary.into(),
            focus,
        }
    }

    /// Find the dialog binary next to the running engine binary, falling back to
    /// its plain name (resolved via `PATH`), with the platform focus manager.
    pub fn discover() -> Self {
        const NAME: &str = "idiolect-review-dialog";
        let beside_engine = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)))
            .filter(|path| path.exists());
        Self {
            binary: beside_engine.unwrap_or_else(|| PathBuf::from(NAME)),
            focus: default_window_focus(),
        }
    }

    /// Spawn the dialog, feed it the transcript, and read back the result.
    fn run_dialog(&self, transcript: &str) -> Option<String> {
        let mut child = Command::new(&self.binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        // Write and close stdin before reading stdout to avoid a pipe deadlock.
        {
            let mut stdin = child.stdin.take()?;
            stdin.write_all(transcript.as_bytes()).ok()?;
        }

        let output = child.wait_with_output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl ReviewDialog for SubprocessReviewDialog {
    fn review(&self, transcript: &str) -> Option<String> {
        // Capture where focus was *before* the dialog steals it.
        let restore_target = self.focus.active_window();
        let result = self.run_dialog(transcript);
        // Hand focus back to the originating window (whether confirmed or
        // cancelled), then let it settle before the caller commits.
        if let Some(window) = restore_target {
            self.focus.restore(window);
            std::thread::sleep(FOCUS_SETTLE);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake used by the engine's review-flow tests.
    pub(crate) struct FakeDialog {
        pub(crate) reply: Option<String>,
    }
    impl ReviewDialog for FakeDialog {
        fn review(&self, _transcript: &str) -> Option<String> {
            self.reply.clone()
        }
    }

    #[test]
    fn fake_dialog_returns_its_reply() {
        let dialog = FakeDialog {
            reply: Some("fixed".to_owned()),
        };
        assert_eq!(dialog.review("raw").as_deref(), Some("fixed"));
        let cancelled = FakeDialog { reply: None };
        assert_eq!(cancelled.review("raw"), None);
    }

    #[test]
    fn subprocess_dialog_echoes_via_cat_on_success() {
        // `cat` mirrors stdin to stdout and exits 0 — i.e. "confirmed, unchanged".
        let dialog = SubprocessReviewDialog::new("cat");
        assert_eq!(dialog.review("hello world").as_deref(), Some("hello world"));
    }

    #[test]
    fn subprocess_dialog_treats_nonzero_exit_as_cancel() {
        // `false` exits non-zero -> cancelled.
        let dialog = SubprocessReviewDialog::new("false");
        assert_eq!(dialog.review("hello"), None);
    }

    #[test]
    fn subprocess_dialog_missing_binary_is_none() {
        let dialog = SubprocessReviewDialog::new("/nonexistent/idiolect-review-dialog-xyz");
        assert_eq!(dialog.review("hello"), None);
    }

    use std::sync::{Arc, Mutex};

    use crate::focus::{WindowFocus, WindowId};

    /// Records focus capture/restore so we can assert the dialog hands focus back.
    #[derive(Clone)]
    struct RecordingFocus {
        active: Option<WindowId>,
        restored: Arc<Mutex<Vec<WindowId>>>,
    }

    impl RecordingFocus {
        fn new(active: Option<WindowId>) -> Self {
            Self {
                active,
                restored: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn restored(&self) -> Vec<WindowId> {
            self.restored.lock().expect("restored mutex").clone()
        }
    }

    impl WindowFocus for RecordingFocus {
        fn active_window(&self) -> Option<WindowId> {
            self.active
        }
        fn restore(&self, window: WindowId) {
            self.restored.lock().expect("restored mutex").push(window);
        }
    }

    #[test]
    fn restores_focus_to_the_captured_window_after_the_dialog() {
        // `cat` confirms unchanged; the captured window must get focus back so the
        // user's commit lands there and Enter works.
        let focus = RecordingFocus::new(Some(42));
        let dialog = SubprocessReviewDialog::with_focus("cat", Box::new(focus.clone()));
        assert_eq!(dialog.review("hello world").as_deref(), Some("hello world"));
        assert_eq!(focus.restored(), vec![42], "focus handed back to the original window");
    }

    #[test]
    fn restores_focus_even_when_the_dialog_is_cancelled() {
        // `false` cancels; focus must still return so the user keeps working.
        let focus = RecordingFocus::new(Some(7));
        let dialog = SubprocessReviewDialog::with_focus("false", Box::new(focus.clone()));
        assert_eq!(dialog.review("hello"), None);
        assert_eq!(focus.restored(), vec![7]);
    }

    #[test]
    fn no_captured_window_means_no_restore_attempt() {
        // No display / nothing focused: capture yields None, so restore is skipped.
        let focus = RecordingFocus::new(None);
        let dialog = SubprocessReviewDialog::with_focus("cat", Box::new(focus.clone()));
        assert_eq!(dialog.review("hi").as_deref(), Some("hi"));
        assert!(focus.restored().is_empty());
    }
}
