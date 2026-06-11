//! Review-dialog abstraction. The engine shows the dictated text in a box it
//! controls, lets the user correct it, and gets the final text back — so the
//! correction is captured regardless of the destination application (it never
//! depends on the app reporting its contents).
//!
//! The dialog is also the live mid-take surface: in review mode the engine
//! streams each pause-snippet into it (`append`), so the user watches the take
//! grow in the SAME window that turns editable at stop (`review`). There is no
//! separate preview window.
//!
//! The concrete GUI is kept behind this trait and, by default, behind a process
//! boundary (`SubprocessReviewDialog`), so the toolkit is swappable with zero
//! impact on the engine and the GUI's heavy dependencies stay out of the IME.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use crate::focus::{default_window_focus, NoopWindowFocus, WindowFocus};

/// After restoring focus, give the window manager + application a moment to
/// process the focus-in and re-establish their input context before the engine
/// commits — otherwise the commit can race the focus hand-back.
const FOCUS_SETTLE: Duration = Duration::from_millis(120);

/// The engine's window onto a review-mode take. Implementations are
/// toolkit-specific and fully interchangeable.
pub trait ReviewDialog: Send + Sync {
    /// Stream one mid-take snippet (joining whitespace already embedded) into
    /// the dialog, opening it in its read-only "listening" state on the first
    /// snippet. Best-effort: must never block dictation on a broken GUI.
    fn append(&self, chunk: &str);
    /// Deliver the take's final text and block until the user confirms
    /// (`Some(edited)`) or cancels (`None`). Reuses the listening dialog when
    /// one is open, else opens fresh (a take can end without any pause).
    fn review(&self, transcript: &str) -> Option<String>;
    /// Tear the dialog down without a result (cancelled take, daemon error).
    fn close(&self);
}

/// Encode a protocol payload for one stdin line: backslash → `\\`,
/// newline → `\n` (mirrored by the dialog binary).
fn escape_payload(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\n', "\\n")
}

struct Running {
    child: Child,
    stdin: ChildStdin,
}

/// Runs an external dialog binary speaking a line protocol on stdin
/// (`append <payload>` / `final <payload>`); the edited text comes back on
/// stdout, and a non-zero exit means "cancelled". This both hides the toolkit
/// and keeps the GUI in its own process (so winit/egui never run inside the
/// async IME).
///
/// The dialog window opens without taking focus (the user is mid-dictation),
/// but once `final` is sent it raises itself; this type captures the active
/// window before that and restores focus afterwards (via [`WindowFocus`]) so
/// the commit lands in the right place and the user can immediately press
/// Enter.
pub struct SubprocessReviewDialog {
    binary: PathBuf,
    args: Vec<String>,
    focus: Box<dyn WindowFocus>,
    state: Mutex<Option<Running>>,
}

impl SubprocessReviewDialog {
    /// Construct with no focus management (capture is a no-op). Used by tests.
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self::with_command(binary, Vec::new(), Box::new(NoopWindowFocus))
    }

    /// Construct with an explicit focus manager (used to inject a fake in tests).
    pub fn with_focus(binary: impl Into<PathBuf>, focus: Box<dyn WindowFocus>) -> Self {
        Self::with_command(binary, Vec::new(), focus)
    }

    /// Full constructor: binary, fixed arguments, focus manager. Tests use
    /// `sh -c <script>` stand-ins so no temp script files are ever exec'd.
    pub fn with_command(
        binary: impl Into<PathBuf>,
        args: Vec<String>,
        focus: Box<dyn WindowFocus>,
    ) -> Self {
        Self {
            binary: binary.into(),
            args,
            focus,
            state: Mutex::new(None),
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
        Self::with_command(
            beside_engine.unwrap_or_else(|| PathBuf::from(NAME)),
            Vec::new(),
            default_window_focus(),
        )
    }

    fn spawn(&self) -> Option<Running> {
        let mut child = Command::new(&self.binary)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        match child.stdin.take() {
            Some(stdin) => Some(Running { child, stdin }),
            None => {
                let _ = child.kill();
                None
            }
        }
    }

    /// Send the final text to the (possibly already-listening) dialog and wait
    /// for the user's verdict.
    fn run_dialog(&self, transcript: &str) -> Option<String> {
        let line = format!("final {}\n", escape_payload(transcript));
        let mut running = match self.state.lock().expect("dialog mutex").take() {
            Some(running) => running,
            None => self.spawn()?,
        };
        let wrote = running
            .stdin
            .write_all(line.as_bytes())
            .and_then(|()| running.stdin.flush());
        if wrote.is_err() {
            // The listening window died mid-take (closed, crashed): the final
            // text must not be lost — show it in a fresh dialog.
            let _ = running.child.kill();
            let _ = running.child.wait();
            running = self.spawn()?;
            let _ = running.stdin.write_all(line.as_bytes());
        }
        let Running { child, stdin } = running;
        // EOF after `final`: the dialog stays open for editing and we wait for
        // its exit (closing first avoids a pipe deadlock).
        drop(stdin);
        let output = child.wait_with_output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl ReviewDialog for SubprocessReviewDialog {
    fn append(&self, chunk: &str) {
        let mut guard = self.state.lock().expect("dialog mutex");
        if guard.is_none() {
            *guard = self.spawn();
        }
        let Some(running) = guard.as_mut() else {
            return; // the dialog is best-effort; dictation must not care
        };
        let wrote = writeln!(running.stdin, "append {}", escape_payload(chunk))
            .and_then(|()| running.stdin.flush());
        if wrote.is_err() {
            // Dead dialog: reap it and let review() (or the next take) respawn.
            if let Some(mut dead) = guard.take() {
                let _ = dead.child.kill();
                let _ = dead.child.wait();
            }
        }
    }

    fn review(&self, transcript: &str) -> Option<String> {
        // Capture where focus was *before* the dialog raises itself on `final`.
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

    fn close(&self) {
        if let Some(mut running) = self.state.lock().expect("dialog mutex").take() {
            let _ = running.child.kill();
            let _ = running.child.wait();
        }
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
        fn append(&self, _chunk: &str) {}
        fn review(&self, _transcript: &str) -> Option<String> {
            self.reply.clone()
        }
        fn close(&self) {}
    }

    /// A dialog stand-in speaking the real line protocol: accumulates appended
    /// snippets and, on `final`, echoes "<appended>|<final>" and confirms.
    const ECHO_SCRIPT: &str = r#"
        acc=""
        while IFS= read -r line; do
            case "$line" in
                "append "*) acc="$acc${line#append }";;
                "final "*) printf '%s|%s' "$acc" "${line#final }"; exit 0;;
            esac
        done
        exit 1
    "#;

    /// Confirms with the final text unchanged (ignores appends).
    const CONFIRM_SCRIPT: &str = r#"
        while IFS= read -r line; do
            case "$line" in
                "final "*) printf '%s' "${line#final }"; exit 0;;
            esac
        done
        exit 1
    "#;

    /// Reads exactly one line: a `final` confirms, anything else dies — used to
    /// simulate the listening window being closed mid-take.
    const DIE_ON_APPEND_SCRIPT: &str = r#"
        IFS= read -r line
        case "$line" in
            "final "*) printf '%s' "${line#final }"; exit 0;;
        esac
        exit 1
    "#;

    fn script_dialog(script: &str) -> SubprocessReviewDialog {
        SubprocessReviewDialog::with_command(
            "sh",
            vec!["-c".to_owned(), script.to_owned()],
            Box::new(NoopWindowFocus),
        )
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
    fn escape_encodes_newlines_and_backslashes() {
        assert_eq!(escape_payload("a\nb"), "a\\nb");
        assert_eq!(escape_payload("a\\nb"), "a\\\\nb");
        assert_eq!(escape_payload("plain"), "plain");
    }

    #[test]
    fn review_without_appends_confirms_the_final_text() {
        // A take with no pause: review() opens the dialog and gets the verdict.
        let dialog = script_dialog(CONFIRM_SCRIPT);
        assert_eq!(dialog.review("hello world").as_deref(), Some("hello world"));
    }

    #[test]
    fn appended_snippets_reach_the_same_dialog_as_the_final() {
        let dialog = script_dialog(ECHO_SCRIPT);
        dialog.append("hello");
        assert!(
            dialog.state.lock().unwrap().is_some(),
            "listening dialog spawned on first snippet"
        );
        dialog.append(" world");
        assert!(dialog.state.lock().unwrap().is_some(), "still one process");
        assert_eq!(
            dialog.review("hello world edited").as_deref(),
            Some("hello world|hello world edited"),
            "the SAME process saw the appends and the final"
        );
        assert!(dialog.state.lock().unwrap().is_none(), "consumed by review");
    }

    #[test]
    fn nonzero_exit_is_cancel() {
        let dialog = script_dialog("exit 1");
        assert_eq!(dialog.review("hello"), None);
    }

    #[test]
    fn missing_binary_never_breaks_the_take() {
        let dialog = SubprocessReviewDialog::new("/nonexistent/idiolect-review-dialog-xyz");
        dialog.append("hello"); // must not panic
        assert!(dialog.state.lock().unwrap().is_none());
        assert_eq!(dialog.review("hello"), None);
        dialog.close(); // idempotent no-op
    }

    #[test]
    fn close_kills_the_listening_dialog() {
        let dialog = script_dialog("while IFS= read -r line; do :; done");
        dialog.append("doomed take");
        assert!(dialog.state.lock().unwrap().is_some());
        dialog.close();
        assert!(dialog.state.lock().unwrap().is_none(), "killed");
        dialog.close(); // idempotent
    }

    #[test]
    fn a_dead_listening_dialog_does_not_lose_the_final_text() {
        // The script dies after its first (append) line — like the user closing
        // the listening window via the WM. review() must respawn with the final.
        let dialog = script_dialog(DIE_ON_APPEND_SCRIPT);
        dialog.append("vanishing");
        // Give the child time to exit so the `final` write hits a closed pipe.
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(dialog.review("kept text").as_deref(), Some("kept text"));
    }

    use std::sync::{Arc, Mutex as StdMutex};

    use crate::focus::{WindowFocus, WindowId};

    /// Records focus capture/restore so we can assert the dialog hands focus back.
    #[derive(Clone)]
    struct RecordingFocus {
        active: Option<WindowId>,
        restored: Arc<StdMutex<Vec<WindowId>>>,
    }

    impl RecordingFocus {
        fn new(active: Option<WindowId>) -> Self {
            Self {
                active,
                restored: Arc::new(StdMutex::new(Vec::new())),
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

    fn focus_dialog(script: &str, focus: RecordingFocus) -> SubprocessReviewDialog {
        SubprocessReviewDialog::with_command(
            "sh",
            vec!["-c".to_owned(), script.to_owned()],
            Box::new(focus),
        )
    }

    #[test]
    fn restores_focus_to_the_captured_window_after_the_dialog() {
        // The captured window must get focus back so the user's commit lands
        // there and Enter works.
        let focus = RecordingFocus::new(Some(42));
        let dialog = focus_dialog(CONFIRM_SCRIPT, focus.clone());
        assert_eq!(dialog.review("hello world").as_deref(), Some("hello world"));
        assert_eq!(
            focus.restored(),
            vec![42],
            "focus handed back to the original window"
        );
    }

    #[test]
    fn restores_focus_even_when_the_dialog_is_cancelled() {
        let focus = RecordingFocus::new(Some(7));
        let dialog = focus_dialog("exit 1", focus.clone());
        assert_eq!(dialog.review("hello"), None);
        assert_eq!(focus.restored(), vec![7]);
    }

    #[test]
    fn no_captured_window_means_no_restore_attempt() {
        // No display / nothing focused: capture yields None, so restore is skipped.
        let focus = RecordingFocus::new(None);
        let dialog = focus_dialog(CONFIRM_SCRIPT, focus.clone());
        assert_eq!(dialog.review("hi").as_deref(), Some("hi"));
        assert!(focus.restored().is_empty());
    }
}
