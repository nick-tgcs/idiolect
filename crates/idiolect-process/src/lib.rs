//! Supervision for helper processes owned by a long-running Idiolect process.
//!
//! GUI helpers are process boundaries: a spawn error or an unexpected exit must
//! be visible in the journal AND as a desktop notification, while an expected
//! exit — the user cancelling, or the session shutting the helper down — stays
//! silent. Getting that line wrong in either direction is a real cost: a missed
//! crash looks like a menu item that does nothing, and a false alert trains the
//! user to ignore the alerts that matter.
//!
//! This lives in its own crate because the daemon is not the only process that
//! launches helpers — the IBus engine launches the review dialog, and losing
//! *that* one silently discards a whole dictated take.

use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

/// Retained stderr. The reader keeps draining past this so a chatty helper can
/// never block on a full pipe; only what we KEEP is capped.
const MAX_STDERR_BYTES: usize = 16 * 1024;
/// Characters of diagnostic in the notification body, before markup escaping.
const MAX_NOTIFICATION_CHARS: usize = 600;
/// How long to wait for the stderr pipe to reach EOF after the child has been
/// reaped. Non-zero because the child's own exit closes its descriptor; longer
/// than instant because a descendant may briefly hold a copy. Bounded because
/// a descendant that holds it forever must not wedge the caller — see
/// [`ObservedChild::wait_with_diagnostic`].
const STDERR_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Signals that mean "the session or its owner is going away", not "the helper
/// crashed". systemd's default `KillMode=control-group` SIGTERMs every process
/// in the daemon's cgroup on restart, so without this a `systemctl --user
/// restart idiolectd` with a Settings window open would tell the user that
/// Settings had failed.
const EXPECTED_SHUTDOWN_SIGNALS: [i32; 3] = [
    1,  // SIGHUP — the session ended
    2,  // SIGINT — Ctrl-C on a foreground run
    15, // SIGTERM — systemctl stop/restart, logout
];

static NEXT_FAILURE_ID: AtomicU64 = AtomicU64::new(1);

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

/// Where helper failures go: the journal, and the user's notification daemon.
#[derive(Clone)]
pub struct FailureReporter {
    notify_command: Arc<str>,
}

impl FailureReporter {
    #[must_use]
    pub fn new(notify_command: impl Into<String>) -> Self {
        Self {
            notify_command: Arc::from(notify_command.into()),
        }
    }

    /// The command failures are reported through. Exposed so a caller can pin
    /// that it wired its OWN configured notifier in: handing a reporter the
    /// wrong string disables every alert it would ever send, silently.
    #[must_use]
    pub fn notify_command(&self) -> &str {
        &self.notify_command
    }

    fn report(&self, component: &str, executable: &Path, diagnostic: &str) {
        let sequence = NEXT_FAILURE_ID.fetch_add(1, Ordering::Relaxed);
        let reference = format!("{}-{sequence}", std::process::id());

        // Every line carries the reference. A helper that logged before dying
        // turns one failure into hundreds of journal entries, and the user was
        // told to go and find this one — so `grep <reference>` has to work on
        // all of them, not just the first.
        eprintln!(
            "helper failure [{reference}] component={component:?} executable={}",
            executable.display()
        );
        for line in diagnostic.lines() {
            eprintln!("helper failure [{reference}] {line}");
        }

        let summary = format!("Idiolect {component} failed");
        let detail = truncate_keeping_tail(&single_line(diagnostic), MAX_NOTIFICATION_CHARS);
        // Escape AFTER truncating: escaping first could cut an entity in half
        // and hand the server `&am`. Expansion is bounded (5x worst case).
        let body = format!(
            "{}\nReference: {reference}. Details: journalctl --user -u idiolectd | grep {reference}",
            escape_markup(&detail)
        );
        notify_user(&self.notify_command, &summary, &body);
    }
}

/// Collapse to one line: control bytes become spaces (a NUL would make
/// `Command::arg` fail, and the notifier swallows that, losing the alert
/// entirely; an ESC would smuggle an ANSI sequence through), then runs of
/// whitespace collapse.
fn single_line(text: &str) -> String {
    let spaced: String = text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keep the END of the diagnostic. A crash reason is the LAST thing a helper
/// writes; keeping the first N characters shows the user its startup banner and
/// hides the reason they were notified.
fn truncate_keeping_tail(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(total - max_chars).collect();
    format!("…{tail}")
}

/// The body is handed to a server that may parse Pango markup — GNOME and KDE
/// both advertise `body-markup` — and the text contains arbitrary helper
/// stderr. Unescaped, a `<span …>` in a log line is silently consumed and the
/// user reads a diagnostic that is not the one produced.
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

struct CapturedStderr {
    text: String,
    truncated: bool,
}

impl CapturedStderr {
    fn empty() -> Self {
        Self {
            text: String::new(),
            truncated: false,
        }
    }
}

fn capture_stderr(mut stderr: impl Read) -> CapturedStderr {
    let mut captured = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 4096];

    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let remaining = MAX_STDERR_BYTES.saturating_sub(captured.len());
                let keep = remaining.min(read);
                captured.extend_from_slice(&buffer[..keep]);
                truncated |= keep < read;
            }
            // A signal interrupted the read; the pipe is still fine. Giving up
            // here would stop draining, and the helper would then block on a
            // full stderr pipe with its stdout never closed.
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }

    CapturedStderr {
        text: String::from_utf8_lossy(&captured).trim().to_owned(),
        truncated,
    }
}

/// A child whose stderr is always drained and whose unexpected termination is
/// reported. Callers keep stdin/stdout for their own wire protocol.
#[must_use = "the child must be waited on so failures are reported"]
pub struct ObservedChild {
    child: Child,
    stderr: Option<mpsc::Receiver<CapturedStderr>>,
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
                );
                return None;
            }
        };
        // The drain runs on its own thread and reports through a channel, so
        // the waiter can give up on it without being stuck to its lifetime.
        let stderr = child.stderr.take().map(|stderr| {
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || {
                let _ = sender.send(capture_stderr(stderr));
            });
            receiver
        });

        Some(Self {
            child,
            stderr,
            reporter,
            component,
            executable,
            waited: false,
        })
    }

    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Wait, reporting unless the child exited in one of the `expected_exit_codes`
    /// or was terminated by a shutdown signal.
    pub fn wait(self, expected_exit_codes: &[i32]) -> Option<ExitStatus> {
        self.wait_with_diagnostic(expected_exit_codes, None)
    }

    /// Tear the child down deliberately, reporting nothing.
    ///
    /// For a helper the caller is closing ON PURPOSE — a cancelled take, a
    /// preview window that is no longer wanted. Whatever the child's exit
    /// status ends up being, we caused it, so an alert would be noise. Distinct
    /// from dropping without waiting, which means a caller forgot.
    pub fn dismiss(mut self) {
        self.waited = true;
        self.child.stdin.take();
        self.child.stdout.take();
        let _ = self.child.kill();
        self.stderr.take();
        let _ = self.child.wait();
    }

    /// Wait after a caller-side protocol failure, folding that diagnostic, the
    /// exit status and the captured stderr into a SINGLE report.
    pub fn wait_with_diagnostic(
        mut self,
        expected_exit_codes: &[i32],
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
                );
                return None;
            }
        };

        // The child is reaped, so its own stderr descriptor is closed. If EOF
        // still hasn't arrived, a descendant is holding a copy — do NOT wait
        // for it. Blocking here would hold the caller's "window open" latch
        // true forever, and the tray item would never reopen.
        let captured = match self.stderr.take() {
            Some(receiver) => receiver
                .recv_timeout(STDERR_DRAIN_GRACE)
                .unwrap_or_else(|_| CapturedStderr::empty()),
            None => CapturedStderr::empty(),
        };

        let expected = match (status.code(), signal_of(&status)) {
            (Some(code), _) => expected_exit_codes.contains(&code),
            (None, Some(signal)) => EXPECTED_SHUTDOWN_SIGNALS.contains(&signal),
            (None, None) => false,
        };

        if !expected || protocol_diagnostic.is_some() {
            let mut diagnostic = protocol_diagnostic.unwrap_or_default().to_owned();
            if !expected {
                if !diagnostic.is_empty() {
                    diagnostic.push_str("; process ");
                }
                diagnostic.push_str(&describe(&status));
            }
            if !captured.text.is_empty() {
                diagnostic.push_str(": ");
                diagnostic.push_str(&captured.text);
                if captured.truncated {
                    diagnostic.push_str(" [stderr truncated]");
                }
            }
            self.reporter
                .report(&self.component, &self.executable, &diagnostic);
        }

        Some(status)
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

impl Drop for ObservedChild {
    fn drop(&mut self) {
        if self.waited {
            return;
        }

        // Stop an abandoned helper before reaping it. Close the protocol pipes
        // and detach the stderr reader so a descendant retaining a pipe cannot
        // block us during unwinding.
        self.child.stdin.take();
        self.child.stdout.take();
        let _ = self.child.kill();
        self.stderr.take();

        let diagnostic = match self.child.wait() {
            Ok(status) => format!("dropped without wait; {}", describe(&status)),
            Err(error) => format!("dropped without wait and could not reap process: {error}"),
        };
        self.reporter
            .report(&self.component, &self.executable, &diagnostic);
    }
}
