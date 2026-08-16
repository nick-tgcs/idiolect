//! Supervision for helper processes owned by a long-running Idiolect process.
//!
//! GUI helpers are process boundaries: a spawn error or an unexpected exit must
//! be recorded durably AND surfaced to the user, while an expected exit — the
//! user cancelling, or the session shutting the helper down — stays silent.
//! Getting that line wrong in either direction is a real cost: a missed crash
//! looks like a menu item that does nothing, and a false alert trains the user
//! to ignore the alerts that matter.
//!
//! This lives in its own crate because the daemon is not the only process that
//! launches helpers — the IBus engine launches the review dialog, and losing
//! *that* one silently discards a whole dictated take. The two processes differ
//! in ways the supervision has to know about, which is why the policies below
//! are per-caller rather than baked in: the daemon's helpers share its cgroup
//! and its journal, while the engine is forked by ibus-daemon with its stderr
//! on `/dev/null`.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The wire contract between a supervisor and the out-of-process GUI dialogs.
///
/// ONE definition, because the two ends live in different crates and a silent
/// drift here is invisible to every test that stubs the dialog with `sh`.
pub mod dialog {
    /// Written to stdout, alone, immediately before exiting [`EXIT_CANCELLED`].
    ///
    /// The exit code alone CANNOT distinguish "the user cancelled" from "the
    /// dialog died": libX11's default I/O-error handler calls `exit(1)` itself
    /// when the X connection drops, so `main` never runs and never gets to
    /// choose a different code. This marker is therefore a POSITIVE signal —
    /// its ABSENCE proves the cancel path was never reached. It begins with
    /// EOT, which no GUI text field will produce.
    pub const CANCELLED_MARKER: &str = "\u{4}cancelled";

    /// The user closed the dialog without choosing.
    pub const EXIT_CANCELLED: i32 = 1;
    /// The dialog never got as far as showing anything; reason on stderr.
    pub const EXIT_UNAVAILABLE: i32 = 2;
}

/// Retained stderr. The reader keeps draining past this so a chatty helper can
/// never block on a full pipe, or take SIGPIPE when we stop listening; only
/// what we KEEP is capped — and we keep the END, because a crash reason is the
/// last thing a helper writes, not the first.
const MAX_STDERR_BYTES: usize = 16 * 1024;
/// Characters of the helper's stderr in the notification body, before markup
/// escaping. The headline is never subject to this — see [`FailureReporter`].
const MAX_NOTIFICATION_CHARS: usize = 600;
/// How long to wait for the stderr pipe to reach EOF after the child has been
/// reaped. Non-zero because the child's own exit closes its descriptor; longer
/// than instant because a descendant may briefly hold a copy. Bounded because a
/// descendant that holds it forever must not wedge the caller.
const STDERR_DRAIN_GRACE: Duration = Duration::from_secs(2);
/// How long an identical alert is suppressed for, where suppression applies.
const REPEAT_SUPPRESSION: Duration = Duration::from_secs(60);

/// Signals that mean "the session or its owner is going away", not "the helper
/// crashed" — but only for a helper that shares our lifecycle. See
/// [`ExpectedExit`].
const SHUTDOWN_SIGNALS: [i32; 3] = [
    1,  // SIGHUP — the session ended
    2,  // SIGINT — Ctrl-C on a foreground run
    15, // SIGTERM — systemctl stop/restart, logout
];

static NEXT_FAILURE_ID: AtomicU64 = AtomicU64::new(1);

/// What counts as a normal end for a particular helper.
#[derive(Clone, Copy)]
pub struct ExpectedExit {
    codes: &'static [i32],
    shutdown_signal_is_normal: bool,
}

impl ExpectedExit {
    /// For a helper that shares our lifecycle — the daemon's tray helpers live
    /// in its cgroup, and systemd's default `KillMode=control-group` SIGTERMs
    /// them on every `systemctl --user restart idiolectd`. That is the session
    /// going away, not the helper failing.
    #[must_use]
    pub const fn shares_our_lifecycle(codes: &'static [i32]) -> Self {
        Self {
            codes,
            shutdown_signal_is_normal: true,
        }
    }

    /// For a helper holding something we cannot recreate — the review dialog is
    /// holding the user's dictated take, and it lives in ibus-daemon's cgroup,
    /// not ours. A signal reaching it is an outside kill, and the user's words
    /// die with it whoever sent it. So no signal is ever "normal" here.
    #[must_use]
    pub const fn holds_user_data(codes: &'static [i32]) -> Self {
        Self {
            codes,
            shutdown_signal_is_normal: false,
        }
    }

    fn permits(self, status: &ExitStatus) -> bool {
        match (status.code(), signal_of(status)) {
            (Some(code), _) => self.codes.contains(&code),
            (None, Some(signal)) => {
                self.shutdown_signal_is_normal && SHUTDOWN_SIGNALS.contains(&signal)
            }
            (None, None) => false,
        }
    }
}

/// Surface a problem to the USER as a desktop notification, via the configured
/// command (`<command> <summary> <body>`; `notify-send` by default, empty =
/// disabled). Telling the user about a failure must never itself cause one:
/// spawn errors are swallowed, and the child is reaped on a detached thread so
/// a slow notifier cannot stall the caller.
pub fn notify_user(command: &str, summary: &str, body: &str) {
    if command.is_empty() {
        return;
    }
    let spawned = Command::new(command)
        .arg(summary)
        .arg(body)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = spawned {
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Where helper failures go: a durable record, and the user's notification
/// daemon.
#[derive(Clone)]
pub struct FailureReporter {
    notify_command: Arc<str>,
    /// What to tell the user to run to see the full diagnostic. `{reference}`
    /// is substituted. Empty means "we have nowhere to send you", and the line
    /// is then omitted rather than pointing at nothing.
    details_hint: Arc<str>,
    /// A durable sink for a process whose stderr goes nowhere. The IBus engine
    /// is forked by ibus-daemon with stderr on `/dev/null`, so for it
    /// `eprintln!` records precisely nothing.
    log_file: Option<Arc<Path>>,
    /// `None` disables repeat suppression, for a helper whose every failure
    /// costs the user something they cannot get back.
    repeat_suppression: Option<Duration>,
    recent: Arc<Mutex<HashMap<String, Instant>>>,
}

impl FailureReporter {
    /// A reporter that records to stderr only, with no "where to look" hint.
    #[must_use]
    pub fn new(notify_command: impl Into<String>) -> Self {
        Self {
            notify_command: Arc::from(notify_command.into()),
            details_hint: Arc::from(""),
            log_file: None,
            repeat_suppression: Some(REPEAT_SUPPRESSION),
            recent: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// For a process whose stderr journald captures under `unit`.
    #[must_use]
    pub fn with_journal_unit(mut self, unit: &str) -> Self {
        self.details_hint = Arc::from(format!("journalctl --user -u {unit} | grep {{reference}}"));
        self
    }

    /// For a process whose stderr is discarded: also append every diagnostic
    /// here, and point the user at it.
    #[must_use]
    pub fn with_log_file(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        self.details_hint = Arc::from(format!("grep {{reference}} {}", path.display()));
        self.log_file = Some(Arc::from(path.as_path()));
        self
    }

    /// Report every occurrence, however repetitive.
    ///
    /// For a helper that is holding the user's data: each failure is another
    /// take thrown away, and telling them about the first one only is the same
    /// silence this crate exists to remove.
    #[must_use]
    pub fn reporting_every_occurrence(mut self) -> Self {
        self.repeat_suppression = None;
        self
    }

    /// The command failures are reported through. Exposed so a caller can pin
    /// that it wired its OWN configured notifier in: handing a reporter the
    /// wrong string disables every alert it would ever send, silently.
    #[must_use]
    pub fn notify_command(&self) -> &str {
        &self.notify_command
    }

    /// `headline` is short, ours, and always survives into the notification —
    /// it carries the exit status and any caller-side protocol failure, which
    /// are the two most actionable facts. `stderr` is the helper's, arbitrary
    /// in size, and is what gets truncated.
    fn report(&self, component: &str, executable: &Path, headline: &str, stderr: &CapturedStderr) {
        // Suppression gates the RECORD too, not just the notification. The
        // recording indicator is launched on every caret update, so a missing
        // overlay binary would otherwise write two lines per keystroke — and
        // the four-hundredth copy of one failure is not more information.
        if !self.should_report(component, headline, &stderr.text) {
            return;
        }

        let sequence = NEXT_FAILURE_ID.fetch_add(1, Ordering::Relaxed);
        let reference = format!("{}-{sequence}", std::process::id());
        self.record(&reference, component, executable, headline, stderr);

        let summary = format!("Idiolect {component} failed");
        let mut body = escape_markup(&single_line(headline));
        if !stderr.text.is_empty() {
            // Escape AFTER truncating: escaping first could cut an entity in
            // half and hand the server `&am`. Expansion is bounded (5x worst).
            let tail = truncate_keeping_tail(&single_line(&stderr.text), MAX_NOTIFICATION_CHARS);
            body.push('\n');
            body.push_str(&escape_markup(&tail));
            if stderr.truncated {
                body.push_str(" [stderr truncated]");
            }
        }
        if !self.details_hint.is_empty() {
            body.push_str("\nReference: ");
            body.push_str(&reference);
            body.push_str(". Details: ");
            body.push_str(&self.details_hint.replace("{reference}", &reference));
        }
        notify_user(&self.notify_command, &summary, &body);
    }

    /// The durable half. Every line carries the reference, because one failure
    /// can be hundreds of lines and the user was told to go and find THIS one.
    fn record(
        &self,
        reference: &str,
        component: &str,
        executable: &Path,
        headline: &str,
        stderr: &CapturedStderr,
    ) {
        let mut record = format!(
            "helper failure [{reference}] component={component:?} executable={} {headline}\n",
            executable.display()
        );
        for line in stderr.text.lines() {
            // The helper's bytes reach a terminal here; an ESC or CR of its
            // choosing must not get to drive one.
            record.push_str(&format!(
                "helper failure [{reference}] {}\n",
                strip_control(line)
            ));
        }

        // `eprintln!` PANICS on a write error, and a Rust process ignores
        // SIGPIPE, so a closed stderr would turn a helper failure into a panic
        // — inside whatever lock the caller happened to be holding.
        let _ = std::io::stderr().write_all(record.as_bytes());

        if let Some(path) = &self.log_file {
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path.as_ref())
                .and_then(|mut file| file.write_all(record.as_bytes()));
        }
    }

    /// Whether this exact failure is due another report.
    ///
    /// Never suppresses on a poisoned lock: losing an alert is worse than
    /// showing one twice.
    fn should_report(&self, component: &str, headline: &str, stderr: &str) -> bool {
        let Some(window) = self.repeat_suppression else {
            return true;
        };
        let Ok(mut recent) = self.recent.lock() else {
            return true;
        };
        let now = Instant::now();
        recent.retain(|_, seen| now.duration_since(*seen) < window);
        // The stderr is part of the identity: the same component failing for a
        // DIFFERENT reason is different news.
        let key = format!("{component}\u{0}{headline}\u{0}{stderr}");
        if recent.contains_key(&key) {
            return false;
        }
        recent.insert(key, now);
        true
    }
}

/// Collapse to one line: control bytes become spaces (a NUL would make
/// `Command::arg` fail, and the notifier swallows that, losing the alert
/// entirely; an ESC would smuggle an ANSI sequence through), then runs of
/// whitespace collapse.
fn single_line(text: &str) -> String {
    strip_control(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Replace anything that could steer a terminal or a notification server with
/// a space, including the bidi overrides that are not `char::is_control`.
fn strip_control(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}') {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

/// Keep the END. A crash reason is the LAST thing a helper writes; keeping the
/// first N characters shows the user its startup banner and hides the reason
/// they were notified.
fn truncate_keeping_tail(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(total - max_chars).collect();
    format!("…{tail}")
}

/// The body may be parsed as Pango markup — GNOME and KDE both advertise
/// `body-markup` — and it contains arbitrary helper stderr. Unescaped, a
/// `<span …>` in a log line is silently consumed and the user reads a
/// diagnostic that is not the one produced.
fn escape_markup(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[derive(Default)]
struct CapturedStderr {
    text: String,
    truncated: bool,
}

/// Drain a helper's stderr, retaining the last [`MAX_STDERR_BYTES`].
///
/// Shared rather than returned, so a caller that gives up waiting still gets
/// what was read: a descendant holding the pipe open must not also cost us the
/// reason the helper died.
fn drain_stderr(mut stderr: impl Read, into: &Arc<Mutex<CapturedStderr>>) {
    let mut retained: Vec<u8> = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 4096];

    loop {
        let read = match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            // A signal interrupted the read; the pipe is still fine. Giving up
            // here would stop draining, and the helper's next write would take
            // SIGPIPE — killing the user's window, not just losing a log line.
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        retained.extend_from_slice(&buffer[..read]);
        if retained.len() > MAX_STDERR_BYTES {
            let excess = retained.len() - MAX_STDERR_BYTES;
            retained.drain(..excess);
            truncated = true;
        }
        if let Ok(mut shared) = into.lock() {
            shared.text = String::from_utf8_lossy(&retained).trim().to_owned();
            shared.truncated = truncated;
        }
    }
}

/// A child whose stderr is always drained and whose unexpected termination is
/// reported. Callers keep stdin/stdout for their own wire protocol.
#[must_use = "the child must be waited on so failures are reported"]
pub struct ObservedChild {
    child: Child,
    stderr: Arc<Mutex<CapturedStderr>>,
    drained: Option<mpsc::Receiver<()>>,
    reporter: FailureReporter,
    component: String,
    executable: PathBuf,
    waited: bool,
}

impl ObservedChild {
    /// Spawn with stderr captured. A spawn failure is reported immediately and
    /// returns `None`, so callers can treat it as "no child" without deciding
    /// how to tell the user.
    pub fn spawn(
        command: &mut Command,
        component: impl Into<String>,
        reporter: FailureReporter,
    ) -> Option<Self> {
        let component = component.into();
        let executable = PathBuf::from(command.get_program());
        command.stderr(Stdio::piped());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                reporter.report(
                    &component,
                    &executable,
                    &format!("could not start: {error}"),
                    &CapturedStderr::default(),
                );
                return None;
            }
        };
        let stderr = Arc::new(Mutex::new(CapturedStderr::default()));
        let drained = child.stderr.take().map(|pipe| {
            let (finished, drained) = mpsc::channel();
            let shared = Arc::clone(&stderr);
            thread::spawn(move || {
                drain_stderr(pipe, &shared);
                let _ = finished.send(());
            });
            drained
        });

        Some(Self {
            child,
            stderr,
            drained,
            reporter,
            component,
            executable,
            waited: false,
        })
    }

    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Wait, reporting unless the child ended the way `expected` describes.
    pub fn wait(self, expected: ExpectedExit) -> Option<ExitStatus> {
        self.wait_with_diagnostic(expected, None)
    }

    /// Wait after a caller-side protocol failure, folding that diagnostic, the
    /// exit status and the captured stderr into a SINGLE report.
    pub fn wait_with_diagnostic(
        mut self,
        expected: ExpectedExit,
        protocol_diagnostic: Option<&str>,
    ) -> Option<ExitStatus> {
        self.waited = true;
        let status = match self.child.wait() {
            Ok(status) => status,
            Err(error) => {
                self.reporter.report(
                    &self.component,
                    &self.executable,
                    &format!("could not observe process exit: {error}"),
                    &CapturedStderr::default(),
                );
                return None;
            }
        };

        // The child is reaped, so its own stderr descriptor is closed. If EOF
        // still hasn't arrived, a descendant is holding a copy — do NOT wait
        // for it, or the caller's "window open" latch stays true forever and
        // the tray item never reopens. Whatever was read by then is still ours.
        if let Some(drained) = self.drained.take() {
            let _ = drained.recv_timeout(STDERR_DRAIN_GRACE);
        }
        let captured = self.captured_stderr();

        let expected_end = expected.permits(&status);
        if !expected_end || protocol_diagnostic.is_some() {
            let mut headline = protocol_diagnostic.unwrap_or_default().to_owned();
            if !expected_end {
                if !headline.is_empty() {
                    headline.push_str("; process ");
                }
                headline.push_str(&describe(&status));
            }
            self.reporter
                .report(&self.component, &self.executable, &headline, &captured);
        }

        Some(status)
    }

    /// Tear the child down deliberately, reporting nothing.
    ///
    /// For a helper the caller is closing ON PURPOSE — a cancelled take, a
    /// preview window that is no longer wanted. Whatever the child's exit
    /// status ends up being, we caused it, so an alert would be noise. Distinct
    /// from dropping without waiting, which means a caller forgot.
    pub fn dismiss(mut self) {
        self.waited = true;
        self.kill_and_reap();
    }

    fn kill_and_reap(&mut self) {
        // Close the protocol pipes and detach the stderr reader so a descendant
        // retaining a pipe cannot block us.
        self.child.stdin.take();
        self.child.stdout.take();
        let _ = self.child.kill();
        self.drained.take();
        let _ = self.child.wait();
    }

    fn captured_stderr(&self) -> CapturedStderr {
        self.stderr.lock().map_or_else(
            |_| CapturedStderr::default(),
            |shared| CapturedStderr {
                text: shared.text.clone(),
                truncated: shared.truncated,
            },
        )
    }
}

impl Drop for ObservedChild {
    fn drop(&mut self) {
        if self.waited {
            return;
        }
        let captured = self.captured_stderr();
        self.kill_and_reap();
        self.reporter.report(
            &self.component,
            &self.executable,
            "dropped without wait",
            &captured,
        );
    }
}

fn signal_of(status: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

fn describe(status: &ExitStatus) -> String {
    match (status.code(), signal_of(status)) {
        (Some(code), _) => format!("exited with status {code}"),
        (None, Some(signal)) => format!("terminated by signal {signal}"),
        (None, None) => "terminated for an unknown reason".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn a_short_diagnostic_is_kept_whole() {
        assert_eq!(truncate_keeping_tail("abc", 10), "abc");
        assert_eq!(truncate_keeping_tail("abcde", 5), "abcde");
    }

    #[test]
    fn truncation_keeps_the_tail_and_marks_the_cut() {
        // The reason a helper died is the LAST thing it wrote.
        let text = format!("{}FATAL", "noise ".repeat(200));

        let kept = truncate_keeping_tail(&text, 20);

        assert!(kept.ends_with("FATAL"), "{kept}");
        assert!(kept.starts_with('…'), "{kept}");
        assert_eq!(kept.chars().count(), 21);
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        let text = "🦀".repeat(10);

        let kept = truncate_keeping_tail(&text, 4);

        assert_eq!(kept.chars().count(), 5);
        assert!(kept.ends_with("🦀🦀🦀🦀"));
    }

    #[test]
    fn markup_is_escaped_so_a_helper_cannot_forge_the_diagnostic() {
        assert_eq!(
            escape_markup("a & b <span>c</span>"),
            "a &amp; b &lt;span&gt;c&lt;/span&gt;"
        );
    }

    #[test]
    fn escaping_after_truncation_never_produces_a_half_entity() {
        let text = "&".repeat(5000);

        let escaped = escape_markup(&truncate_keeping_tail(&text, MAX_NOTIFICATION_CHARS));

        // Escaping first and truncating second could hand the server `&am`.
        // Every `&` here must be the start of a complete entity.
        assert_eq!(
            escaped.matches('&').count(),
            escaped.matches("&amp;").count(),
            "a half-written entity reached the body"
        );
        assert_eq!(escaped.matches("&amp;").count(), MAX_NOTIFICATION_CHARS);
    }

    #[test]
    fn control_bytes_and_bidi_overrides_are_neutralised() {
        // A NUL makes `Command::arg` fail, which the notifier swallows — the
        // alert would vanish. ESC drives a terminal. RLO reorders the text the
        // user reads.
        let folded = single_line("a\u{0}b\u{1b}[31mc\u{202e}d\ne");

        assert!(!folded.contains('\u{0}'), "{folded:?}");
        assert!(!folded.contains('\u{1b}'), "{folded:?}");
        assert!(!folded.contains('\u{202e}'), "{folded:?}");
        assert_eq!(folded, "a b [31mc d e");
    }

    #[test]
    fn a_signal_is_described_separately_from_an_exit_code() {
        assert_eq!(
            describe(&ExitStatus::from_raw(0x0100)),
            "exited with status 1"
        );
        assert_eq!(describe(&ExitStatus::from_raw(9)), "terminated by signal 9");
    }

    #[test]
    fn a_shared_lifecycle_helper_tolerates_shutdown_signals_but_not_a_crash() {
        let policy = ExpectedExit::shares_our_lifecycle(&[0]);

        assert!(policy.permits(&ExitStatus::from_raw(0)));
        assert!(policy.permits(&ExitStatus::from_raw(15))); // SIGTERM
        assert!(!policy.permits(&ExitStatus::from_raw(11))); // SIGSEGV
        assert!(!policy.permits(&ExitStatus::from_raw(0x0100))); // exit 1
    }

    #[test]
    fn a_data_holding_helper_treats_every_signal_as_loss() {
        let policy = ExpectedExit::holds_user_data(&[0, 1]);

        assert!(policy.permits(&ExitStatus::from_raw(0)));
        assert!(policy.permits(&ExitStatus::from_raw(0x0100))); // exit 1 = cancel
        assert!(
            !policy.permits(&ExitStatus::from_raw(15)),
            "an outside SIGTERM still means the user's take is gone"
        );
    }

    #[test]
    fn the_retained_stderr_is_the_tail_not_the_head() {
        let mut noisy = "warning: nothing important\n".repeat(2000);
        noisy.push_str("FATAL: no GPU adapter found\n");
        let shared = Arc::new(Mutex::new(CapturedStderr::default()));

        drain_stderr(noisy.as_bytes(), &shared);

        let captured = shared.lock().expect("captured stderr");
        assert!(captured.truncated);
        assert!(captured.text.len() <= MAX_STDERR_BYTES);
        assert!(
            captured.text.contains("FATAL: no GPU adapter found"),
            "the reason was dropped in favour of the banner"
        );
    }

    #[test]
    fn a_quiet_helper_is_not_marked_truncated() {
        let shared = Arc::new(Mutex::new(CapturedStderr::default()));

        drain_stderr("just this\n".as_bytes(), &shared);

        let captured = shared.lock().expect("captured stderr");
        assert_eq!(captured.text, "just this");
        assert!(!captured.truncated);
    }

    #[test]
    fn a_details_hint_substitutes_the_reference() {
        let reporter = FailureReporter::new("").with_journal_unit("idiolectd");

        assert_eq!(
            reporter.details_hint.replace("{reference}", "42-1"),
            "journalctl --user -u idiolectd | grep 42-1"
        );
    }

    #[test]
    fn suppression_is_keyed_on_the_reason_not_just_the_component() {
        let reporter = FailureReporter::new("");

        assert!(reporter.should_report("Review dialog", "could not start", ""));
        assert!(
            !reporter.should_report("Review dialog", "could not start", ""),
            "the identical failure should be held back"
        );
        assert!(
            reporter.should_report("Review dialog", "exited with status 2", ""),
            "the same helper failing for a DIFFERENT reason is different news"
        );
        assert!(
            reporter.should_report("Review dialog", "could not start", "Glutin BadAttribute"),
            "the same headline with different stderr is different news"
        );
    }

    #[test]
    fn a_reporter_can_be_told_to_report_every_occurrence() {
        let reporter = FailureReporter::new("").reporting_every_occurrence();

        assert!(reporter.should_report("Review dialog", "could not start", ""));
        assert!(
            reporter.should_report("Review dialog", "could not start", ""),
            "a helper holding the user's take must report every loss"
        );
    }
}
