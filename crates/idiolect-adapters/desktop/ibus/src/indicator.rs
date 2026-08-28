//! Caret-badge abstraction. While a take is in progress the engine shows a badge
//! next to the caret naming the phase — recording, transcribing, loading the
//! model — and this hides the concrete GUI behind a trait (and, by default, a
//! process boundary) so it is swappable and its dependencies stay out of the IME.
//!
//! The overlay process is long-lived: [`SubprocessIndicator::warm_up`] starts it
//! hidden at engine startup and `hide` unmaps it rather than killing it, so a
//! take does not pay for building a GL window before the badge can appear.

use std::io::Write;
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Mutex;

use idiolect_ipc::messages::ActivityPhase;
use idiolect_process::{ExpectedExit, FailureReporter, ObservedChild};

/// Shows the recording indicator at a caret position, repositions it and
/// re-labels it while it's already showing, and hides it. All calls are
/// idempotent.
pub trait RecordingIndicator: Send + Sync {
    /// Show at, or move to, the given caret screen position, displaying `phase`.
    fn show(&self, x: i32, y: i32, phase: ActivityPhase);
    fn hide(&self);
}

/// The token for a phase in the overlay's stdin protocol.
///
/// The overlay binary is deliberately free of idiolect dependencies (it is a tiny
/// GUI process), so it parses these words rather than sharing this type. Treat
/// them as a wire protocol: both sides pin them in tests.
pub(crate) fn phase_word(phase: ActivityPhase) -> &'static str {
    match phase {
        ActivityPhase::Transcribing => "transcribing",
        ActivityPhase::LoadingModel => "loading",
        // Idle IS the hide instruction: the overlay outlives the take and just
        // unmaps itself, so it is ready to appear instantly on the next one.
        ActivityPhase::Idle => "hidden",
        ActivityPhase::Recording => "recording",
    }
}

// Only the engine positions the overlay, so without that feature these are used
// solely by the tests below.
#[cfg(any(feature = "ibus-engine", test))]
/// Where the badge goes when neither a caret nor a focused window is known.
/// Reached whenever window geometry is unavailable — a Wayland-only or headless
/// session gets [`NoopWindowFocus`](crate::focus::NoopWindowFocus), whose anchor
/// is always `None` — so it is a real fallback, not a theoretical one.
pub(crate) const LAST_RESORT_ANCHOR: (i32, i32) = (400, 400);

/// Whether a caret learned while `current` was focused is still meaningful now
/// that `next` has focus. Only within one input context — a different context is
/// a different window or application, whose cursor is somewhere else entirely.
#[cfg(any(feature = "ibus-engine", test))]
pub(crate) fn caret_survives_context_change(current: Option<&str>, next: &str) -> bool {
    current == Some(next)
}

/// Decide where to put the badge, in order of how much we actually know.
///
/// A reported caret is the truth. Failing that, the focused window gives a spot
/// that is at least on the window the user is typing into. The engine used to
/// skip straight to a hardcoded (400, 400) and keep it until an app volunteered
/// a caret rect — so on a fresh engine, or in any app that never reports one,
/// the badge appeared in a corner of the screen far from the user's cursor and
/// read as not appearing at all.
#[cfg(any(feature = "ibus-engine", test))]
pub(crate) fn resolve_anchor(
    caret: Option<(i32, i32)>,
    focused_window: Option<(i32, i32)>,
) -> (i32, i32) {
    caret.or(focused_window).unwrap_or(LAST_RESORT_ANCHOR)
}

struct Running {
    child: ObservedChild,
    stdin: ChildStdin,
}

/// Launches an external overlay binary once and streams `"x y phase"` lines to
/// its stdin, so it tracks the text caret and unmaps itself between takes.
/// Keeping it out-of-process means the overlay's GUI stack never runs inside the
/// async IME.
pub struct SubprocessIndicator {
    binary: PathBuf,
    reporter: FailureReporter,
    state: Mutex<Option<Running>>,
    /// Exactly what was last put on the wire, so an unchanged state can be
    /// suppressed and a hide can reuse the last known caret rather than
    /// teleporting the (invisible) window to 0,0. `None` until the first line.
    last: Mutex<Option<(i32, i32, ActivityPhase)>>,
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
            last: Mutex::new(None),
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

impl SubprocessIndicator {
    /// Build the overlay now, hidden, so the first take does not pay for it.
    ///
    /// Creating the overlay's GL window costs roughly a third of a second —
    /// process start, context creation, font atlas, first frame. Paid at the
    /// stop of a take that is happening anyway, that is invisible; paid at the
    /// START of one, it is the gap between pressing the key and seeing the badge,
    /// which is exactly the moment the indicator exists to cover.
    pub fn warm_up(&self) {
        self.stream(0, 0, ActivityPhase::Idle);
    }

    /// Send one `"x y phase"` line, launching the overlay if it is not running.
    fn stream(&self, x: i32, y: i32, phase: ActivityPhase) {
        let word = phase_word(phase);
        let mut last = self.last.lock().expect("indicator last mutex");
        let mut guard = self.state.lock().expect("indicator mutex");
        if guard.is_some() && *last == Some((x, y, phase)) {
            // The overlay is already in this exact state. Worth suppressing
            // rather than writing anyway: `sync_indicator` runs on every key
            // event and every caret report, and apps report a caret rect on
            // every keystroke — so an idle, hidden overlay would be told to hide
            // again, and woken to draw nothing, for every character typed.
            return;
        }
        *last = Some((x, y, phase));
        if let Some(running) = guard.as_mut() {
            // Already showing — stream the new caret position and phase so it
            // follows the caret and re-labels in place. Re-launching to change the
            // badge would flash the overlay and re-map a window that is carefully
            // configured to stay out of the focus rotation.
            let wrote =
                writeln!(running.stdin, "{x} {y} {word}").and_then(|()| running.stdin.flush());
            if wrote.is_ok() {
                return;
            }
            // The pipe is broken, so the overlay died on its own — a
            // deliberate teardown goes through `hide`. Reap it so the failure
            // is reported, then fall through and start a fresh one; leaving the
            // corpse in place meant no overlay for the rest of the recording.
            if let Some(dead) = guard.take() {
                drop(dead.stdin);
                let _ = dead.child.wait(ExpectedExit::shares_our_lifecycle(&[0]));
            }
        }
        let mut command = Command::new(&self.binary);
        command
            .arg(x.to_string())
            .arg(y.to_string())
            .arg(word)
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
}

impl RecordingIndicator for SubprocessIndicator {
    fn show(&self, x: i32, y: i32, phase: ActivityPhase) {
        self.stream(x, y, phase);
    }

    /// Unmap the overlay but LEAVE IT RUNNING, ready for the next take.
    ///
    /// In the shipped engine the overlay is torn down by ITS OWN end-of-stdin:
    /// `ibus::run` parks on `pending()` and only ever exits by signal, so the
    /// `Drop` below runs in tests and never in production. The third exit is the
    /// broken-pipe path in [`Self::stream`], which reaps an overlay that died on
    /// its own and starts a fresh one.
    fn hide(&self) {
        // Nothing running means nothing to hide — and a hide must never START
        // the overlay. `sync_indicator` calls this on every key event and every
        // caret report while no take is in progress, so on a machine where the
        // overlay cannot spawn (no binary, no X server, a GL failure) a hide
        // that were willing to launch would fork once per character typed and
        // re-alert the user every 60 s. Only `show` and `warm_up` may start it.
        if self.state.lock().expect("indicator mutex").is_none() {
            return;
        }
        // Scoped read: `stream` takes this same lock.
        let last = *self.last.lock().expect("indicator last mutex");
        let (x, y) = last.map_or((0, 0), |(x, y, _)| (x, y));
        self.stream(x, y, ActivityPhase::Idle);
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
    fn an_overlay_that_dies_on_its_own_is_reported_and_then_replaced() {
        // A crash AFTER a successful spawn left the dead child in place: the
        // next `show` wrote into a broken pipe and ignored the error, so the
        // overlay never came back for the rest of the recording and the user
        // was never told why.
        let recorder = idiolect_test_support::notifications::NotificationRecorder::new();
        let directory = tempfile::tempdir().expect("temporary overlay directory");
        let overlay = directory.path().join("overlay");
        idiolect_test_support::notifications::write_executable_script(
            &overlay,
            "#!/bin/sh\nprintf 'GL context lost\\n' >&2\nexit 23\n",
        );
        let indicator = SubprocessIndicator::with_notifier(&overlay, recorder.command().to_owned());

        indicator.show(30, 30, ActivityPhase::Recording);
        // Let the overlay die before the reposition that finds the dead pipe.
        std::thread::sleep(std::time::Duration::from_millis(200));
        indicator.show(40, 50, ActivityPhase::Recording);

        let alert = recorder.wait();
        assert!(
            alert.contains("Idiolect Recording indicator failed"),
            "{alert}"
        );
        assert!(alert.contains("status 23"), "{alert}");
        assert!(alert.contains("GL context lost"), "{alert}");
    }

    #[test]
    fn tearing_the_engine_down_while_showing_does_not_alert() {
        let recorder = idiolect_test_support::notifications::NotificationRecorder::new();
        let indicator = SubprocessIndicator::with_notifier("cat", recorder.command().to_owned());
        indicator.show(30, 30, ActivityPhase::Recording);
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
    fn show_repositions_one_long_lived_process() {
        // `cat` stands in for the overlay: it reads stdin (our position stream)
        // and stays alive across takes.
        let indicator = SubprocessIndicator::new("cat");
        indicator.show(30, 30, ActivityPhase::Recording);
        assert!(indicator.state.lock().unwrap().is_some(), "spawned");
        // Showing again repositions via stdin rather than respawning.
        indicator.show(40, 50, ActivityPhase::Recording);
        assert!(
            indicator.state.lock().unwrap().is_some(),
            "still one process"
        );
        indicator.hide();
        indicator.hide(); // idempotent
        assert!(
            indicator.state.lock().unwrap().is_some(),
            "still warm after hiding"
        );
    }

    /// An overlay stand-in that records every line the engine streams to it.
    fn recording_overlay(directory: &std::path::Path) -> std::path::PathBuf {
        let script = directory.join("overlay");
        let log = directory.join("lines");
        idiolect_test_support::notifications::write_executable_script(
            &script,
            &format!(
                "#!/bin/sh\nprintf '%s %s %s\\n' \"$1\" \"$2\" \"$3\" >> {log}\ncat >> {log}\n",
                log = log.display()
            ),
        );
        // `write_executable_script` runs the script once, with no arguments, to
        // confirm it is executable — discard that probe's line.
        std::fs::write(&log, "").expect("reset overlay log");
        script
    }

    fn overlay_lines(directory: &std::path::Path) -> Vec<String> {
        // The overlay is a separate process; give it a moment to flush.
        for _ in 0..50 {
            let text = std::fs::read_to_string(directory.join("lines")).unwrap_or_default();
            if text.lines().count() >= 2 {
                return text.lines().map(str::to_owned).collect();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::fs::read_to_string(directory.join("lines"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn the_phase_is_streamed_to_a_running_overlay_rather_than_respawning_it() {
        // Recording → transcribing happens mid-take, in front of the user. Killing
        // and relaunching the window to change the badge would flash the overlay
        // off and on, and would re-map a window we have gone to some trouble to
        // keep out of the focus rotation.
        let directory = tempfile::tempdir().expect("temporary overlay directory");
        let indicator = SubprocessIndicator::new(recording_overlay(directory.path()));

        indicator.show(30, 40, ActivityPhase::Recording);
        let spawned = indicator.state.lock().unwrap().is_some();
        assert!(spawned, "the overlay launched");
        indicator.show(30, 40, ActivityPhase::Transcribing);
        assert!(
            indicator.state.lock().unwrap().is_some(),
            "still the same overlay process",
        );

        let lines = overlay_lines(directory.path());
        assert_eq!(lines.first().map(String::as_str), Some("30 40 recording"));
        assert_eq!(lines.get(1).map(String::as_str), Some("30 40 transcribing"));
        indicator.hide();
    }

    #[test]
    fn hiding_keeps_the_overlay_warm_instead_of_killing_it() {
        // Killing the overlay at every stop meant the NEXT take paid a fresh
        // process start, GL context and first paint — measured at ~0.35 s before
        // the badge appeared, which reads as "it didn't show". The process now
        // outlives the take and is told to unmap itself.
        let directory = tempfile::tempdir().expect("temporary overlay directory");
        let indicator = SubprocessIndicator::new(recording_overlay(directory.path()));

        indicator.show(30, 40, ActivityPhase::Recording);
        indicator.hide();
        assert!(
            indicator.state.lock().unwrap().is_some(),
            "the overlay process must survive a hide",
        );

        let lines = overlay_lines(directory.path());
        assert_eq!(lines.first().map(String::as_str), Some("30 40 recording"));
        assert_eq!(
            lines.get(1).map(String::as_str),
            Some("30 40 hidden"),
            "hide streams the hidden phase at the last known caret",
        );
    }

    #[test]
    fn hiding_an_overlay_that_is_not_running_never_starts_one() {
        // `sync_indicator` calls `hide` on EVERY key event and every caret
        // report while no take is in progress. If the overlay cannot be spawned
        // at all — no binary, no X server, a GL failure — nothing is ever left
        // in `state`, so a hide that is willing to launch retries the spawn for
        // every character the user types, forever, and the failure notifier
        // re-alerts every 60 s. There is also nothing to hide when nothing is
        // showing: only `show` and `warm_up` may start the process.
        let directory = tempfile::tempdir().expect("temporary overlay directory");
        let indicator = SubprocessIndicator::new(recording_overlay(directory.path()));

        indicator.hide();
        indicator.hide();

        assert!(
            indicator.state.lock().unwrap().is_none(),
            "a hide must never launch the overlay",
        );
    }

    #[test]
    fn an_unchanged_state_is_not_re_streamed_to_the_overlay() {
        // `sync_indicator` runs on every key event and every caret report, and
        // apps report a caret rect on EVERY KEYSTROKE. Since `hide` became a
        // streamed line rather than a kill, that put a pipe write and a wake-up
        // of the overlay's paint loop on the hot path of the user simply typing
        // with no take in progress. Only real changes go on the wire — but a
        // MOVE is a real change, so caret tracking must survive the suppression.
        let directory = tempfile::tempdir().expect("temporary overlay directory");
        let indicator = SubprocessIndicator::new(recording_overlay(directory.path()));

        indicator.warm_up();
        indicator.hide(); // already hidden: nothing to say
        indicator.hide();
        indicator.show(30, 40, ActivityPhase::Recording);
        indicator.show(30, 40, ActivityPhase::Recording); // same spot, same phase
        indicator.show(31, 40, ActivityPhase::Recording); // moved: must be sent

        let lines = overlay_lines(directory.path());
        assert_eq!(
            lines,
            ["0 0 hidden", "30 40 recording", "31 40 recording"],
            "only changes reach the overlay, and every change does",
        );
    }

    #[test]
    fn warming_up_starts_the_overlay_hidden_before_any_take() {
        // So the very FIRST Super+T is instant too, not just later ones.
        let directory = tempfile::tempdir().expect("temporary overlay directory");
        let indicator = SubprocessIndicator::new(recording_overlay(directory.path()));
        indicator.warm_up();
        assert!(
            indicator.state.lock().unwrap().is_some(),
            "warm_up must launch the overlay",
        );
        let lines = overlay_lines(directory.path());
        assert_eq!(
            lines.first().map(String::as_str),
            Some("0 0 hidden"),
            "a warm start must not put anything on screen",
        );
        indicator.hide();
    }

    #[test]
    fn switching_input_context_discards_the_old_windows_caret() {
        // Each app/window is a separate IBus input context. The engine used to
        // keep the last caret it was told about forever, so after switching
        // windows the badge appeared at the PREVIOUS window's cursor — on screen,
        // inside the new window even, but nowhere near where the user was
        // actually typing. Indistinguishable from "it doesn't show".
        assert!(!caret_survives_context_change(
            Some("/org/freedesktop/IBus/Engine/idiolect/6"),
            "/org/freedesktop/IBus/Engine/idiolect/7",
        ));
        // The same context re-focusing is NOT a change: Electron apps flap
        // focus_out/focus_in on one context many times a second, and dropping the
        // caret on each would make the badge jump about mid-take.
        assert!(caret_survives_context_change(
            Some("/org/freedesktop/IBus/Engine/idiolect/6"),
            "/org/freedesktop/IBus/Engine/idiolect/6",
        ));
        // Nothing known yet: there is no caret to keep either way.
        assert!(!caret_survives_context_change(None, "/whatever"));
    }

    #[test]
    fn a_reported_caret_always_wins() {
        assert_eq!(
            resolve_anchor(Some((1927, 1309)), Some((10, 20))),
            (1927, 1309)
        );
    }

    #[test]
    fn without_a_caret_the_badge_goes_to_the_focused_window_not_a_made_up_spot() {
        // The engine used to start with a hardcoded (400, 400) caret and keep it
        // until an app reported a real one. On a 2560x1440 screen that put the
        // badge in the upper left while the user watched their cursor — so the
        // indicator looked like it simply never appeared. It fires on every
        // engine start, and an app that never reports a caret rect (some
        // terminals, some browsers) never clears it.
        assert_eq!(resolve_anchor(None, Some((640, 900))), (640, 900));
    }

    #[test]
    fn with_nothing_known_at_all_it_still_shows_somewhere_sane() {
        // Neither a caret nor a focused window means there is nothing to anchor
        // to; showing the badge somewhere beats silently showing nothing, which
        // is the failure this whole path exists to avoid.
        assert_eq!(resolve_anchor(None, None), LAST_RESORT_ANCHOR);
    }

    #[test]
    fn phase_words_are_the_exact_tokens_the_overlay_binary_parses() {
        // The overlay is dependency-free by design, so it re-parses these words
        // instead of sharing a type. They are a wire protocol between two
        // binaries: pinned on both sides, changed on neither alone.
        assert_eq!(phase_word(ActivityPhase::Recording), "recording");
        assert_eq!(phase_word(ActivityPhase::Transcribing), "transcribing");
        assert_eq!(phase_word(ActivityPhase::LoadingModel), "loading");
        // Idle IS the hide signal now that the overlay outlives the take.
        assert_eq!(phase_word(ActivityPhase::Idle), "hidden");
    }
}
