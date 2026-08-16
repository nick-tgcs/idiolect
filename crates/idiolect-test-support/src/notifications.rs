//! A stand-in for the desktop notifier, for tests that assert on what the
//! daemon told the user.
//!
//! The notify contract is `<command> <summary> <body>`, so the recorder has to
//! be a real executable file — a `sh -c` stand-in cannot take positional
//! arguments. Writing a script and exec'ing it straight away is racy: a sibling
//! test thread's `fork` inherits the still-open write descriptor, and the exec
//! then fails with `ETXTBSY`. The caller cannot retry, because the production
//! notifier deliberately swallows spawn errors so dictation never fails just
//! because telling the user about a failure failed. So the constructor closes
//! that window itself, by exec'ing the script until it takes.
//!
//! That probe doubles as a positive control: a test asserting that NOTHING was
//! notified proves nothing unless the recorder could have recorded.

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Generous on purpose: CI runs these under `cargo llvm-cov` instrumentation,
/// which is far slower than a plain test run.
const DEADLINE: Duration = Duration::from_secs(10);

/// Writes `body` as an executable script and blocks until it can actually be
/// exec'd, so callers never see the `ETXTBSY` race described above.
///
/// # Panics
/// If the script cannot be written, or never becomes exec'able in time.
pub fn write_executable_script(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write executable script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod executable script");

    let deadline = Instant::now() + DEADLINE;
    loop {
        match Command::new(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let _ = child.wait();
                return;
            }
            // Someone else's fork still holds a write descriptor to this file.
            // It closes on their exec, which is imminent — spin briefly.
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {
                assert!(
                    Instant::now() < deadline,
                    "{} never became executable",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("could not exec {}: {error}", path.display()),
        }
    }
}

/// A notifier stand-in that appends one `<summary>|<body>` line per call.
pub struct NotificationRecorder {
    _directory: tempfile::TempDir,
    command: String,
    log: PathBuf,
}

impl NotificationRecorder {
    /// # Panics
    /// If the recorder cannot be created, or cannot record its own probe.
    #[must_use]
    pub fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary notifier directory");
        let log = directory.path().join("notifications.log");
        let notifier = directory.path().join("notify");
        // Newlines in the body are folded to spaces so that one invocation is
        // always exactly one line: bodies legitimately span lines, and a
        // line-per-notification log is what makes `records()` a count of
        // notifications rather than a count of newlines.
        write_executable_script(
            &notifier,
            &format!(
                "#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"$(printf '%s' \"$2\" | tr '\\n' ' ')\" >> \"{}\"\n",
                log.display()
            ),
        );

        // The probe exec inside `write_executable_script` ran the recorder with
        // no arguments, so the log must exist now. Asserting it is the positive
        // control for every `log_path().exists()` assertion downstream.
        let probe = std::fs::read_to_string(&log).expect("recorder did not record its own probe");
        assert!(
            !probe.is_empty(),
            "notification recorder produced no output for its probe"
        );
        std::fs::remove_file(&log).expect("clear the probe from the notification log");

        Self {
            _directory: directory,
            command: notifier.to_string_lossy().into_owned(),
            log,
        }
    }

    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    #[must_use]
    pub fn log_path(&self) -> &Path {
        &self.log
    }

    /// Every notification recorded so far, one per line.
    #[must_use]
    pub fn records(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .map(|log| log.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Blocks until a whole notification line has been recorded, then returns
    /// the log. Waiting for the newline avoids reading a half-written record.
    ///
    /// # Panics
    /// If nothing is recorded before the deadline.
    #[must_use]
    pub fn wait(&self) -> String {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Ok(contents) = std::fs::read_to_string(&self.log) {
                if contents.ends_with('\n') {
                    return contents;
                }
            }
            assert!(Instant::now() < deadline, "notification was not emitted");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Default for NotificationRecorder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_recorder_has_recorded_nothing_but_has_proven_it_can() {
        let recorder = NotificationRecorder::new();

        // The probe ran and was cleared: the log is gone, yet we know the
        // script execs and writes — which is what makes an "expected no
        // notification" assertion meaningful.
        assert!(!recorder.log_path().exists());
        assert!(recorder.records().is_empty());
    }

    #[test]
    fn it_records_each_invocation_as_one_line() {
        let recorder = NotificationRecorder::new();

        for body in ["first", "second"] {
            let status = Command::new(recorder.command())
                .arg("Idiolect")
                .arg(body)
                .status()
                .expect("run recorder");
            assert!(status.success());
        }

        assert_eq!(
            recorder.records(),
            vec!["Idiolect|first".to_owned(), "Idiolect|second".to_owned()]
        );
    }

    #[test]
    fn wait_blocks_until_a_notification_actually_arrives() {
        let recorder = NotificationRecorder::new();
        let command = recorder.command().to_owned();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            Command::new(command)
                .arg("Idiolect")
                .arg("late")
                .status()
                .expect("run recorder");
        });

        let recorded = recorder.wait();
        writer.join().expect("recorder writer");

        assert!(recorded.ends_with('\n'));
        assert_eq!(recorded.trim_end(), "Idiolect|late");
    }

    #[test]
    fn a_script_becomes_executable_even_while_another_process_holds_it_open_for_write() {
        // This is the ETXTBSY race the retry exists for: a sibling test's fork
        // inherits an open write descriptor, and `execve` fails until that
        // descriptor closes. Without the retry the caller's notifier simply
        // never runs — which is the ~9% flake this helper was written to end.
        let directory = tempfile::tempdir().expect("temporary script directory");
        let script = directory.path().join("held");
        let ready = directory.path().join("opened");
        let marker = directory.path().join("ran");
        std::fs::write(&script, "#!/bin/sh\n").expect("seed the script");

        // Append, not truncate, so the holder cannot clobber what we write.
        let mut holder = Command::new("sh")
            .args([
                "-c",
                &format!(
                    "exec 3>>'{}'; : > '{}'; sleep 1",
                    script.display(),
                    ready.display()
                ),
            ])
            .spawn()
            .expect("spawn the descriptor holder");
        let deadline = Instant::now() + DEADLINE;
        while !ready.exists() {
            assert!(Instant::now() < deadline, "holder never opened the script");
            std::thread::sleep(Duration::from_millis(5));
        }

        write_executable_script(
            &script,
            &format!("#!/bin/sh\nprintf 'ran\\n' >> \"{}\"\n", marker.display()),
        );

        let _ = holder.wait();
        assert!(
            marker.exists(),
            "gave up on a busy script instead of retrying"
        );
    }

    #[test]
    fn a_written_script_is_executable_before_the_helper_returns() {
        let directory = tempfile::tempdir().expect("temporary script directory");
        let script = directory.path().join("marker");
        let marker = directory.path().join("ran");

        write_executable_script(
            &script,
            &format!("#!/bin/sh\nprintf 'ran\\n' >> \"{}\"\n", marker.display()),
        );

        assert!(marker.exists(), "probe never executed the script");
    }
}
