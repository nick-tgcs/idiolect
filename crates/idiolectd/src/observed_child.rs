//! Shared supervision for daemon-owned helper processes.
//!
//! GUI helpers are process boundaries: a spawn error or unexpected exit must be
//! visible in the daemon journal and as a desktop notification, while expected
//! user cancellation remains silent.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

const MAX_STDERR_BYTES: usize = 16 * 1024;
const MAX_NOTIFICATION_CHARS: usize = 600;
static NEXT_FAILURE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(crate) struct FailureReporter {
    notify_command: Arc<str>,
}

impl FailureReporter {
    pub(crate) fn new(notify_command: impl Into<String>) -> Self {
        Self {
            notify_command: Arc::from(notify_command.into()),
        }
    }

    fn report(&self, component: &str, executable: &Path, diagnostic: &str) {
        let sequence = NEXT_FAILURE_ID.fetch_add(1, Ordering::Relaxed);
        let reference = format!("{}-{sequence}", std::process::id());
        eprintln!(
            "helper failure [{reference}] component={component:?} executable={}: {diagnostic}",
            executable.display()
        );

        let summary = format!("Idiolect {component} failed");
        let compact = diagnostic.split_whitespace().collect::<Vec<_>>().join(" ");
        let detail = truncate_chars(&compact, MAX_NOTIFICATION_CHARS);
        let body = format!(
            "{detail}\nReference: {reference}. Details: journalctl --user -u idiolectd -n 100"
        );
        crate::adapters::notify_user(&self.notify_command, &summary, &body);
    }
}

struct CapturedStderr {
    text: String,
    truncated: bool,
}

fn capture_stderr(mut stderr: impl Read) -> CapturedStderr {
    let mut captured = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 4096];

    while let Ok(read) = stderr.read(&mut buffer) {
        if read == 0 {
            break;
        }
        let remaining = MAX_STDERR_BYTES.saturating_sub(captured.len());
        let keep = remaining.min(read);
        captured.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }

    CapturedStderr {
        text: String::from_utf8_lossy(&captured).trim().to_owned(),
        truncated,
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

/// A child whose stderr is always drained and whose unexpected termination is
/// reported. Callers retain access to stdin/stdout for their wire protocol.
#[must_use = "the child must be waited on so failures are reported"]
pub(crate) struct ObservedChild {
    child: Child,
    stderr_reader: Option<JoinHandle<CapturedStderr>>,
    reporter: FailureReporter,
    component: String,
    executable: PathBuf,
    waited: bool,
}

impl ObservedChild {
    /// Spawn after forcing stderr capture. Spawn failures are reported
    /// immediately and return `None`.
    pub(crate) fn spawn(
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
        let stderr_reader = child
            .stderr
            .take()
            .map(|stderr| thread::spawn(move || capture_stderr(stderr)));

        Some(Self {
            child,
            stderr_reader,
            reporter,
            component,
            executable,
            waited: false,
        })
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Wait and report unless the process exits with one of the explicitly
    /// allowed codes. A non-zero code that writes stderr is still a failure,
    /// which distinguishes a documented silent cancellation from a crash.
    pub(crate) fn wait(self, allowed_exit_codes: &[i32]) -> Option<ExitStatus> {
        self.wait_with_diagnostic(allowed_exit_codes, None)
    }

    /// Wait after a caller-side protocol failure, combining that diagnostic
    /// with the process status and captured stderr in a single report.
    pub(crate) fn wait_with_diagnostic(
        mut self,
        allowed_exit_codes: &[i32],
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
        let captured = self
            .stderr_reader
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or(CapturedStderr {
                text: String::new(),
                truncated: false,
            });

        let allowed_status = status
            .code()
            .is_some_and(|code| allowed_exit_codes.contains(&code));
        let status_failed =
            !allowed_status || (status.code() != Some(0) && !captured.text.is_empty());
        if status_failed || protocol_diagnostic.is_some() {
            let mut diagnostic = protocol_diagnostic.unwrap_or_default().to_owned();
            if status_failed {
                if !diagnostic.is_empty() {
                    diagnostic.push_str("; process ");
                }
                diagnostic.push_str(&match status.code() {
                    Some(code) => format!("exited with status {code}"),
                    None => "terminated by a signal".to_owned(),
                });
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

impl Drop for ObservedChild {
    fn drop(&mut self) {
        if self.waited {
            return;
        }

        // Stop an abandoned helper before reaping it. Close protocol pipes and
        // detach the stderr reader so descendants retaining a pipe cannot block
        // the daemon during unwinding.
        self.child.stdin.take();
        self.child.stdout.take();
        let _ = self.child.kill();
        self.stderr_reader.take();

        let diagnostic = match self.child.wait() {
            Ok(status) => match status.code() {
                Some(code) => format!("dropped without wait; exited with status {code}"),
                None => "dropped without wait; terminated by a signal".to_owned(),
            },
            Err(error) => format!("dropped without wait and could not reap process: {error}"),
        };
        self.reporter
            .report(&self.component, &self.executable, &diagnostic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::NotificationRecorder;
    use std::time::{Duration, Instant};

    #[test]
    fn notification_detail_is_unicode_safe_and_bounded() {
        let detail = format!("{}{}", "a".repeat(MAX_NOTIFICATION_CHARS), "🦀");
        let truncated = truncate_chars(&detail, MAX_NOTIFICATION_CHARS);
        assert!(truncated.ends_with('…'));
        assert_eq!(truncated.chars().count(), MAX_NOTIFICATION_CHARS + 1);
    }

    #[test]
    fn stderr_capture_keeps_a_bounded_prefix_while_draining_everything() {
        let input = vec![b'x'; MAX_STDERR_BYTES + 2048];
        let captured = capture_stderr(input.as_slice());
        assert_eq!(captured.text.len(), MAX_STDERR_BYTES);
        assert!(captured.truncated);
    }

    #[test]
    fn caller_protocol_failure_is_reported_once_after_the_child_is_reaped() {
        let recorder = NotificationRecorder::new();
        let reporter = FailureReporter::new(recorder.command().to_owned());
        let mut command = Command::new("sh");
        command.args(["-c", "exit 0"]);
        let child =
            ObservedChild::spawn(&mut command, "Protocol helper", reporter).expect("spawn helper");

        let status = child
            .wait_with_diagnostic(&[0], Some("could not read helper output: injected error"))
            .expect("wait for helper");

        assert!(status.success());
        let alert = recorder.wait();
        assert!(alert.contains("could not read helper output"), "{alert}");
        assert_eq!(alert.matches("Idiolect Protocol helper failed|").count(), 1);
    }

    #[test]
    fn dropping_without_wait_reaps_and_reports_without_blocking_on_the_child() {
        let recorder = NotificationRecorder::new();
        let reporter = FailureReporter::new(recorder.command().to_owned());
        let mut command = Command::new("sleep");
        command.arg("0.5");
        let child =
            ObservedChild::spawn(&mut command, "Abandoned helper", reporter).expect("spawn helper");

        let started = Instant::now();
        drop(child);
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "drop blocked until the abandoned child exited"
        );

        let alert = recorder.wait();
        assert!(alert.contains("Abandoned helper"), "{alert}");
        assert!(alert.contains("without wait"), "{alert}");
    }
}
