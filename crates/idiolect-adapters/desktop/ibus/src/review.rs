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

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use idiolect_process::dialog::{CANCELLED_MARKER, EXIT_CANCELLED};
use idiolect_process::{ExpectedExit, FailureReporter, ObservedChild};

use crate::focus::{default_window_focus, WindowFocus};
// Only the test-only constructors need the no-op focus manager.
#[cfg(test)]
use crate::focus::NoopWindowFocus;

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
    /// (`Some(edited)`). `None` means the take is being discarded — either the
    /// user cancelled, or the dialog failed and the user has been told, since
    /// the caller cannot tell those apart and must drop the take either way.
    /// Reuses the listening dialog when one is open, else opens fresh (a take
    /// can end without any pause).
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
    child: ObservedChild,
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
    /// For the FINAL attempt: each failure there is another take discarded, so
    /// every one of them is reported.
    final_reporter: FailureReporter,
    /// For the mid-take preview: it re-spawns on every pause snippet, and none
    /// of those failures lose anything — the whole transcript is still handed
    /// to the final attempt. Reporting each one is a notification storm.
    preview_reporter: FailureReporter,
    state: Mutex<Option<Running>>,
}

impl SubprocessReviewDialog {
    /// Construct with no focus management (capture is a no-op). Used by tests.
    #[cfg(test)]
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self::with_command(binary, Vec::new(), Box::new(NoopWindowFocus))
    }

    /// Construct with an explicit focus manager (used to inject a fake in tests).
    #[cfg(test)]
    pub fn with_focus(binary: impl Into<PathBuf>, focus: Box<dyn WindowFocus>) -> Self {
        Self::with_command(binary, Vec::new(), focus)
    }

    /// Full constructor: binary, fixed arguments, focus manager. Tests use
    /// `sh -c <script>` stand-ins so no temp script files are ever exec'd.
    #[cfg(test)]
    pub fn with_command(
        binary: impl Into<PathBuf>,
        args: Vec<String>,
        focus: Box<dyn WindowFocus>,
    ) -> Self {
        Self::with_notifier(binary, args, focus, String::new())
    }

    /// As [`Self::with_command`], plus the command used to tell the user when
    /// the dialog fails.
    pub fn with_notifier(
        binary: impl Into<PathBuf>,
        args: Vec<String>,
        focus: Box<dyn WindowFocus>,
        notify_command: impl Into<String>,
    ) -> Self {
        let reporter = FailureReporter::new(notify_command)
            .with_log_file(crate::notify::diagnostics_log_path());
        Self {
            binary: binary.into(),
            args,
            focus,
            // The engine's stderr is /dev/null, so both diagnostics have to go
            // to a file the user can actually be sent to.
            final_reporter: reporter.clone().reporting_every_occurrence(),
            preview_reporter: reporter,
            state: Mutex::new(None),
        }
    }

    /// The notify command this dialog reports failures through.
    #[cfg(test)]
    pub(crate) fn notify_command(&self) -> &str {
        self.final_reporter.notify_command()
    }

    /// Find the dialog binary next to the running engine binary, falling back to
    /// its plain name (resolved via `PATH`), with the platform focus manager.
    pub fn discover(notify_command: &str) -> Self {
        const NAME: &str = "idiolect-review-dialog";
        let beside_engine = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)))
            .filter(|path| path.exists());
        Self::with_notifier(
            beside_engine.unwrap_or_else(|| PathBuf::from(NAME)),
            Vec::new(),
            default_window_focus(),
            notify_command,
        )
    }

    /// Exit 1 counts as a cancel ONLY when the dialog said so on its way out.
    /// libX11's I/O-error handler exits 1 from underneath `main`, so without
    /// the marker an exit of 1 means the dialog DIED. Shared by both paths that
    /// reap a dialog, so they cannot drift apart.
    fn expected_exit_for(stdout: &str) -> ExpectedExit {
        if stdout == CANCELLED_MARKER {
            ExpectedExit::holds_user_data(&[0, EXIT_CANCELLED])
        } else {
            ExpectedExit::holds_user_data(&[0])
        }
    }

    fn spawn(&self, reporter: &FailureReporter) -> Option<Running> {
        let mut command = Command::new(&self.binary);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = ObservedChild::spawn(&mut command, "Review dialog", reporter.clone())?;
        match child.child_mut().stdin.take() {
            Some(stdin) => Some(Running { child, stdin }),
            // No stdin means no protocol; we are closing it on purpose.
            None => {
                child.dismiss();
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
            None => self.spawn(&self.final_reporter)?,
        };
        let wrote = running
            .stdin
            .write_all(line.as_bytes())
            .and_then(|()| running.stdin.flush());
        if wrote.is_err() {
            // The listening window died mid-take (closed, crashed): the final
            // text must not be lost — show it in a fresh dialog. We are the
            // ones tearing this one down, so it is not a failure to report.
            running.child.dismiss();
            running = self.spawn(&self.final_reporter)?;
            let _ = running.stdin.write_all(line.as_bytes());
        }
        let Running { mut child, stdin } = running;
        // EOF after `final`: the dialog stays open for editing and we wait for
        // its exit (closing first avoids a pipe deadlock).
        drop(stdin);
        let mut edited = Vec::new();
        let read_error = match child.child_mut().stdout.take() {
            Some(mut pipe) => pipe
                .read_to_end(&mut edited)
                .err()
                .map(|error| format!("could not read the edited text: {error}")),
            None => Some("could not read the edited text: stdout pipe unavailable".to_owned()),
        };
        let text = String::from_utf8_lossy(&edited).into_owned();
        // Exit 1 counts as a cancel ONLY when the dialog said so on its way
        // out. libX11's I/O-error handler exits 1 from underneath `main`, so
        // without the marker an exit of 1 means the dialog DIED — and the take
        // it was holding is about to be discarded. The user has to be told, or
        // their words vanish without a trace.
        let status =
            child.wait_with_diagnostic(Self::expected_exit_for(&text), read_error.as_deref())?;
        if read_error.is_some() || status.code() != Some(0) {
            return None;
        }
        Some(text)
    }
}

impl ReviewDialog for SubprocessReviewDialog {
    fn append(&self, chunk: &str) {
        let mut guard = self.state.lock().expect("dialog mutex");
        if guard.is_none() {
            *guard = self.spawn(&self.preview_reporter);
        }
        let Some(running) = guard.as_mut() else {
            return; // the dialog is best-effort; dictation must not care
        };
        let wrote = writeln!(running.stdin, "append {}", escape_payload(chunk))
            .and_then(|()| running.stdin.flush());
        if wrote.is_err() {
            // Dead dialog: reap it and let review() (or the next take) respawn.
            // Same shape as the overlay: a broken pipe means the window went
            // away by itself, so reap it rather than dismissing it. The exit
            // code alone is not enough — an X11 death exits 1 too — so read
            // what it wrote and apply the same marker rule as the final path.
            if let Some(mut dead) = guard.take() {
                drop(dead.stdin);
                let mut farewell = Vec::new();
                if let Some(mut pipe) = dead.child.child_mut().stdout.take() {
                    let _ = pipe.read_to_end(&mut farewell);
                }
                let _ = dead
                    .child
                    .wait(Self::expected_exit_for(&String::from_utf8_lossy(&farewell)));
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
        if let Some(running) = self.state.lock().expect("dialog mutex").take() {
            running.child.dismiss();
        }
    }
}

impl Drop for SubprocessReviewDialog {
    fn drop(&mut self) {
        // The engine going away takes any open window with it. That is us
        // closing the dialog, not the dialog failing — alerting on shutdown
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

    fn notifying_dialog(script: &str, notify_command: &str) -> SubprocessReviewDialog {
        SubprocessReviewDialog::with_notifier(
            "sh",
            vec!["-c".to_owned(), script.to_owned()],
            Box::new(NoopWindowFocus),
            notify_command.to_owned(),
        )
    }

    /// The cancel marker as a POSIX `printf` escape, for the `sh` stand-ins.
    const CANCEL_ESCAPE: &str = "\\004cancelled";

    #[test]
    fn a_preview_that_cannot_open_alerts_once_not_once_per_pause() {
        // `append` respawns on every pause snippet while `state` is None, and
        // none of those failures lose anything — the whole transcript still
        // reaches the final attempt. One notification per pause is a storm.
        let recorder = idiolect_test_support::notifications::NotificationRecorder::new();
        let dialog = SubprocessReviewDialog::with_notifier(
            "/nonexistent/idiolect-review-dialog-xyz",
            Vec::new(),
            Box::new(NoopWindowFocus),
            recorder.command().to_owned(),
        );

        for snippet in ["one", "two", "three", "four"] {
            dialog.append(snippet);
        }

        let alert = recorder.wait();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            recorder.records().len(),
            1,
            "four failed previews produced {} notifications: {alert}",
            recorder.records().len()
        );
    }

    #[test]
    fn a_listening_window_that_dies_without_the_marker_is_reported() {
        // An X11 death during listening exits 1 with no marker, exactly like a
        // cancel. Permitting 1 unconditionally on this path classified it as
        // the user closing the preview and said nothing.
        let recorder = idiolect_test_support::notifications::NotificationRecorder::new();
        let dialog = notifying_dialog(
            "read line; printf 'X connection to :0 broken\\n' >&2; exit 1",
            recorder.command(),
        );

        dialog.append("first");
        // Let it read the snippet and die before the next write finds the pipe.
        std::thread::sleep(Duration::from_millis(300));
        dialog.append("second");

        let alert = recorder.wait();
        assert!(alert.contains("Idiolect Review dialog failed"), "{alert}");
        assert!(alert.contains("X connection"), "{alert}");
    }

    #[test]
    fn dying_with_the_cancel_code_but_no_marker_alerts_instead_of_binning_the_take() {
        // This is what an X connection loss looks like: libX11's I/O-error
        // handler calls exit(1) itself, so the dialog never reaches its cancel
        // path and never writes the marker. Judged on the exit code alone this
        // is indistinguishable from Cancel, and the engine throws the take away.
        let recorder = idiolect_test_support::notifications::NotificationRecorder::new();
        let dialog = notifying_dialog(
            "printf 'X connection to :0 broken (explicit kill or server shutdown)\\n' >&2; exit 1",
            recorder.command(),
        );

        assert_eq!(dialog.review("every word of the take"), None);

        let alert = recorder.wait();
        assert!(alert.contains("Idiolect Review dialog failed"), "{alert}");
        assert!(alert.contains("X connection"), "{alert}");
    }

    #[test]
    fn a_dialog_that_cannot_start_alerts_instead_of_silently_binning_the_take() {
        // The engine maps `None` onto cancel_reviewed(), which discards the
        // take. So a dialog that CRASHED was indistinguishable from the user
        // pressing Cancel: everything they had just dictated disappeared, with
        // nothing in the journal and nothing on screen.
        let recorder = idiolect_test_support::notifications::NotificationRecorder::new();
        let dialog = notifying_dialog(
            "printf 'Glutin BadAttribute\n' >&2; exit 2",
            recorder.command(),
        );

        assert_eq!(dialog.review("every word of the take"), None);

        let alert = recorder.wait();
        assert!(alert.contains("Idiolect Review dialog failed"), "{alert}");
        assert!(alert.contains("Glutin BadAttribute"), "{alert}");
    }

    #[test]
    fn tearing_the_engine_down_with_a_window_open_does_not_alert() {
        // The engine owns the dialog for its whole life. When the engine goes
        // away, any still-open window goes with it — that is us closing it, not
        // it failing, and an alert on shutdown is pure noise.
        let recorder = idiolect_test_support::notifications::NotificationRecorder::new();
        let dialog = notifying_dialog("sleep 30", recorder.command());
        dialog.append("listening");
        assert!(
            dialog.state.lock().unwrap().is_some(),
            "the listening window should be open"
        );

        let started = std::time::Instant::now();
        drop(dialog);

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "teardown waited for the window instead of closing it: {:?}",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            recorder.records().is_empty(),
            "engine teardown alerted the user: {:?}",
            recorder.records()
        );
    }

    #[test]
    fn cancelling_the_dialog_stays_silent() {
        let recorder = idiolect_test_support::notifications::NotificationRecorder::new();
        let dialog = notifying_dialog(
            &format!("printf '{}'; exit 1", CANCEL_ESCAPE),
            recorder.command(),
        );

        assert_eq!(dialog.review("every word of the take"), None);

        std::thread::sleep(Duration::from_millis(150));
        // The recorder proved it can record when constructed, so an empty log
        // means nothing was notified rather than a broken recorder.
        assert!(
            recorder.records().is_empty(),
            "cancelling alerted the user: {:?}",
            recorder.records()
        );
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
    fn the_documented_cancel_exit_code_yields_no_text() {
        // Exit 1 WITH the marker is the user cancelling; without it, or with
        // any other code, the dialog failed and the user is told.
        let dialog = script_dialog(&format!("printf '{CANCEL_ESCAPE}'; exit 1"));
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
