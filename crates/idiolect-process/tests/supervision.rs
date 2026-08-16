//! Contract for helper-process supervision, exercised through the public API
//! with real child processes — the failure modes here (a full pipe, a signal,
//! a descendant holding a descriptor) do not exist for an in-process fake.
//!
//! Coverage levels for this behaviour, per CONTRIBUTING.md:
//!   * unit — the pure helpers in this crate's own `mod tests` (truncation,
//!     escaping, control folding, the exit-status policy, stderr retention).
//!   * integration — this file: real processes, real pipes, real signals, a
//!     real notifier binary and a real log file.
//!   * e2e — `crates/idiolectd/tests/tray_helper_failure_e2e.rs` drives a real
//!     tray activation over a private D-Bus session, through a real daemon, to
//!     a real helper, and asserts the user's configured notifier is invoked;
//!     `tests/exit_contract.rs` in each dialog crate runs the SHIPPED binaries
//!     and pins the exit codes both ends of the protocol depend on.

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use idiolect_process::{ExpectedExit, FailureReporter, ObservedChild};
use idiolect_test_support::notifications::NotificationRecorder;

/// Long enough for a loaded CI runner under coverage instrumentation, short
/// enough that a genuine hang fails the test instead of the suite timeout.
const WAIT_BUDGET: Duration = Duration::from_secs(30);
/// Daemon-style policy: a clean exit only, shutdown signals tolerated.
const CLEAN_EXIT: ExpectedExit = ExpectedExit::shares_our_lifecycle(&[0]);

/// stdout is nulled, not inherited: a helper that outlives the test would
/// otherwise hold the test binary's stdout open, and `cargo test` (whose stdout
/// CI pipes) blocks until it closes. That turned a 2.5 s run into 60 s.
fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    command
}

/// Runs `wait` off-thread so a supervision bug fails as a timeout with a
/// message, rather than hanging the whole test binary.
fn wait_within_budget(child: ObservedChild, expected: ExpectedExit) {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = child.wait(expected);
        let _ = done.send(());
    });
    finished
        .recv_timeout(WAIT_BUDGET)
        .expect("wait() never returned — supervision is blocking the caller");
}

#[test]
fn an_expected_exit_code_is_silent_even_when_the_helper_wrote_to_stderr() {
    // The retention dialog exits 1 to mean "user cancelled", and GL stacks
    // routinely print driver noise on a perfectly healthy run. Treating
    // "non-zero AND wrote something" as a crash made every cancel alert.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell("printf 'glx: failed to create dri3 screen\\n' >&2; exit 1");

    let child = ObservedChild::spawn(&mut command, "Retention dialog", reporter).expect("spawn");
    wait_within_budget(child, ExpectedExit::shares_our_lifecycle(&[0, 1]));

    std::thread::sleep(Duration::from_millis(200));
    assert!(
        recorder.records().is_empty(),
        "user cancellation alerted the user: {:?}",
        recorder.records()
    );
}

#[test]
fn an_unexpected_exit_code_is_reported_with_its_stderr() {
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell("printf 'no GPU adapter found\\n' >&2; exit 23");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let alert = recorder.wait();
    assert!(alert.contains("Idiolect Settings failed"), "{alert}");
    assert!(alert.contains("status 23"), "{alert}");
    assert!(alert.contains("no GPU adapter found"), "{alert}");
}

#[test]
fn a_shutdown_signal_is_silent_for_a_helper_that_shares_our_lifecycle() {
    // systemd's default KillMode=control-group SIGTERMs open helper windows
    // when the daemon restarts; that is not the helper failing.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut terminated = shell("kill -TERM $$; sleep 5");

    let child = ObservedChild::spawn(&mut terminated, "Settings", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    std::thread::sleep(Duration::from_millis(200));
    assert!(
        recorder.records().is_empty(),
        "a daemon restart alerted the user: {:?}",
        recorder.records()
    );
}

#[test]
fn the_same_signal_is_reported_for_a_helper_holding_the_users_data() {
    // The review dialog lives in ibus-daemon's cgroup, not ours, so a SIGTERM
    // reaching it is an outside kill — and the take it was holding is gone.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut terminated = shell("kill -TERM $$; sleep 5");

    let child = ObservedChild::spawn(&mut terminated, "Review dialog", reporter).expect("spawn");
    wait_within_budget(child, ExpectedExit::holds_user_data(&[0, 1]));

    let alert = recorder.wait();
    assert!(alert.contains("terminated by signal 15"), "{alert}");
}

#[test]
fn a_crash_signal_is_always_reported() {
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut aborted = shell("kill -ABRT $$; sleep 5");

    let child = ObservedChild::spawn(&mut aborted, "Settings", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let alert = recorder.wait();
    assert!(alert.contains("terminated by signal 6"), "{alert}");
}

#[test]
fn a_chatty_helper_runs_to_completion_because_stderr_is_drained_past_the_cap() {
    // Only the RETAINED window is capped; the reader must keep consuming. If it
    // stopped at the cap it would drop the pipe, and the helper's next write
    // would take SIGPIPE — killing the user's window mid-work, not merely
    // truncating a log. So the pin is that the helper reaches its final action.
    let recorder = NotificationRecorder::new();
    let directory = tempfile::tempdir().expect("temporary marker directory");
    let marker = directory.path().join("completed");
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell(&format!(
        "i=0; while [ $i -lt 4000 ]; do \
         printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n' >&2; \
         i=$((i+1)); done; printf 'done' > \"{}\"; exit 9",
        marker.display()
    ));

    let child = ObservedChild::spawn(&mut command, "Dashboard", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    assert!(
        marker.exists(),
        "the helper died before finishing — its stderr was not drained"
    );
    let alert = recorder.wait();
    assert!(alert.contains("stderr truncated"), "{alert}");
}

#[test]
fn the_reason_survives_a_helper_far_noisier_than_the_retained_window() {
    // 260 KB of banner then the reason, against a 16 KiB retained window. If
    // retention keeps the HEAD, the reason is gone before truncation even runs
    // — and then it is missing from the notification AND the record.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell(
        "i=0; while [ $i -lt 4000 ]; do \
         printf 'libEGL warning: DRI3: failed to open the dri3 device xxxxxxx\\n' >&2; \
         i=$((i+1)); done; \
         printf 'FATAL: no GPU adapter found - install mesa-vulkan-drivers\\n' >&2; exit 4",
    );

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let alert = recorder.wait();
    assert!(
        alert.contains("install mesa-vulkan-drivers"),
        "the actionable last line never survived retention: {alert}"
    );
}

#[test]
fn the_exit_status_survives_however_much_the_helper_wrote() {
    // The status and any caller-side protocol failure are ours, short, and the
    // most actionable things in the alert. Folding them into one string with
    // the helper's stderr and then keeping the tail drops them entirely.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell(
        "i=0; while [ $i -lt 60 ]; do printf 'libEGL warning: DRI3 probe failed\\n' >&2; \
         i=$((i+1)); done; exit 12",
    );

    let child = ObservedChild::spawn(&mut command, "Retention dialog", reporter).expect("spawn");
    let status = child
        .wait_with_diagnostic(
            ExpectedExit::shares_our_lifecycle(&[0, 1]),
            Some("could not read dialog output: broken pipe"),
        )
        .expect("wait");

    assert_eq!(status.code(), Some(12));
    let alert = recorder.wait();
    assert!(alert.contains("could not read dialog output"), "{alert}");
    assert!(alert.contains("status 12"), "{alert}");
}

#[test]
fn helper_stderr_cannot_smuggle_markup_into_the_notification_body() {
    // GNOME and KDE advertise `body-markup`, so an unescaped span would be
    // consumed and the user would read a diagnostic that was never produced.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command =
        shell("printf 'renderer=<span foreground=\"red\">FAIL</span>\\n' >&2; exit 5");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let alert = recorder.wait();
    assert!(
        alert.contains("&lt;span"),
        "markup was not escaped: {alert}"
    );
    assert!(
        !alert.contains("<span"),
        "raw markup reached the body: {alert}"
    );
}

#[test]
fn an_entity_is_never_cut_in_half_on_the_way_to_the_notification() {
    // Escaping before truncating hands the server `&am` and the whole body is
    // then rejected as invalid markup.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell(
        "i=0; while [ $i -lt 200 ]; do \
         printf '&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&&\\n' >&2; i=$((i+1)); done; exit 6",
    );

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let alert = recorder.wait();
    assert_eq!(
        alert.matches('&').count(),
        alert.matches("&amp;").count(),
        "a half-written entity reached the body: {alert}"
    );
}

#[test]
fn a_nul_byte_in_stderr_still_produces_a_notification() {
    // `Command::arg` rejects a NUL, and the notifier swallows spawn errors —
    // so an unsanitised NUL loses the alert entirely, and silently.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell("printf 'boom\\000more\\n' >&2; exit 7");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let alert = recorder.wait();
    assert!(alert.contains("status 7"), "{alert}");
}

#[test]
fn a_spawn_failure_is_reported_with_the_os_error() {
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = Command::new("/nonexistent/idiolect-helper-xyz");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter);

    assert!(child.is_none());
    let alert = recorder.wait();
    assert!(alert.contains("could not start"), "{alert}");
    assert!(alert.contains("No such file or directory"), "{alert}");
}

#[test]
fn a_protocol_failure_and_a_bad_exit_are_folded_into_one_report() {
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell("printf 'disk full\\n' >&2; exit 12");

    let child = ObservedChild::spawn(&mut command, "Retention dialog", reporter).expect("spawn");
    let status = child
        .wait_with_diagnostic(
            ExpectedExit::shares_our_lifecycle(&[0, 1]),
            Some("could not read dialog output: broken pipe"),
        )
        .expect("wait");

    assert_eq!(status.code(), Some(12));
    let alert = recorder.wait();
    // One record carrying BOTH halves is the property: two separate reports
    // for one failure would double-alert the user.
    assert_eq!(recorder.records().len(), 1, "{alert}");
    assert!(alert.contains("could not read dialog output"), "{alert}");
    assert!(alert.contains("status 12"), "{alert}");
    assert!(alert.contains("disk full"), "{alert}");
}

#[test]
fn a_descendant_holding_stderr_cannot_wedge_the_caller_or_cost_us_the_reason() {
    // Before the drain was bounded, joining the reader meant waiting for every
    // holder of the pipe — so a stray grandchild pinned the launcher's "window
    // open" latch. Giving up must not also throw away what was already read.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    // The helper writes, then exits at once; the backgrounded sleep inherits
    // stderr and holds the pipe open past the drain grace.
    let mut command = shell("printf 'FATAL: adapter lost\\n' >&2; sleep 5 & exit 3");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    let started = std::time::Instant::now();
    wait_within_budget(child, CLEAN_EXIT);

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "wait() waited for a descendant, not the child: {:?}",
        started.elapsed()
    );
    let alert = recorder.wait();
    assert!(
        alert.contains("FATAL: adapter lost"),
        "giving up on the drain also discarded what it had read: {alert}"
    );
}

#[test]
fn a_dismissed_child_is_reaped_without_alerting() {
    // Closing a helper on purpose — a cancelled take, a preview the user shut —
    // is not a failure. Reporting it would train the user to ignore the alerts
    // that do matter.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell("sleep 30");

    let child = ObservedChild::spawn(&mut command, "Review dialog", reporter).expect("spawn");
    let started = std::time::Instant::now();
    child.dismiss();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "dismiss blocked on the child instead of killing it"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        recorder.records().is_empty(),
        "a deliberate teardown alerted the user: {:?}",
        recorder.records()
    );
}

#[test]
fn dropping_without_waiting_kills_reaps_and_reports() {
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell("sleep 30");

    let child = ObservedChild::spawn(&mut command, "Abandoned helper", reporter).expect("spawn");
    let started = std::time::Instant::now();
    drop(child);

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "drop blocked on the abandoned child"
    );
    let alert = recorder.wait();
    assert!(alert.contains("Abandoned helper"), "{alert}");
    assert!(alert.contains("without wait"), "{alert}");
}

#[test]
fn an_identical_failure_does_not_report_over_and_over() {
    // One broken thing fails on EVERY attempt: the recording indicator is
    // launched on every caret update, so an unsuppressed failure would write
    // two lines per keystroke and toast the user per utterance.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());

    for _ in 0..4 {
        let mut command = Command::new("/nonexistent/idiolect-helper-xyz");
        assert!(ObservedChild::spawn(&mut command, "Settings", reporter.clone()).is_none());
    }

    let alert = recorder.wait();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        recorder.records().len(),
        1,
        "four identical failures produced {} notifications: {alert}",
        recorder.records().len()
    );
}

#[test]
fn the_same_helper_failing_for_a_different_reason_is_reported_again() {
    // Suppression keyed on the component alone would swallow this — and for
    // the review dialog that is a second take thrown away in silence.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());

    let mut missing = Command::new("/nonexistent/idiolect-helper-xyz");
    assert!(ObservedChild::spawn(&mut missing, "Review dialog", reporter.clone()).is_none());
    let mut crashing = shell("printf 'no GPU adapter\\n' >&2; exit 4");
    let child = ObservedChild::spawn(&mut crashing, "Review dialog", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while recorder.records().len() < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "the same helper's DIFFERENT failure was suppressed: {:?}",
            recorder.records()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_reporter_can_be_told_to_report_every_occurrence() {
    // Each failure of a take-holding helper is another take discarded; telling
    // the user about the first one only is the silence this crate removes.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned()).reporting_every_occurrence();

    for _ in 0..3 {
        let mut command = Command::new("/nonexistent/idiolect-review-dialog-xyz");
        assert!(ObservedChild::spawn(&mut command, "Review dialog", reporter.clone()).is_none());
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while recorder.records().len() < 3 {
        assert!(
            std::time::Instant::now() < deadline,
            "only {} of 3 lost takes were reported",
            recorder.records().len()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn turning_notifications_off_does_not_turn_the_record_off() {
    // `notify_command = ""` means "no desktop toasts", not "lose the evidence".
    let directory = tempfile::tempdir().expect("temporary log directory");
    let log = directory.path().join("engine.log");
    let reporter = FailureReporter::new(String::new()).with_log_file(&log);
    let mut command = shell("printf 'FATAL: adapter lost\\n' >&2; exit 3");

    let child = ObservedChild::spawn(&mut command, "Review dialog", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let recorded = std::fs::read_to_string(&log).expect("the failure was not recorded");
    assert!(recorded.contains("Review dialog"), "{recorded}");
    assert!(recorded.contains("FATAL: adapter lost"), "{recorded}");
}

#[test]
fn every_recorded_line_carries_the_reference_the_user_is_told_to_grep() {
    // One failure can be hundreds of lines. The body sends the user after a
    // single reference, so it has to appear on all of them, not just the first.
    let recorder = NotificationRecorder::new();
    let directory = tempfile::tempdir().expect("temporary log directory");
    let log = directory.path().join("engine.log");
    let reporter = FailureReporter::new(recorder.command().to_owned()).with_log_file(&log);
    let mut command = shell(
        "i=0; while [ $i -lt 40 ]; do printf 'line %s\\n' \"$i\" >&2; i=$((i+1)); done; exit 8",
    );

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let alert = recorder.wait();
    let reference = alert
        .split("Reference: ")
        .nth(1)
        .and_then(|rest| rest.split('.').next())
        .expect("body carries a reference")
        .to_owned();
    let recorded = std::fs::read_to_string(&log).expect("recorded");
    let lines: Vec<&str> = recorded.lines().collect();
    assert!(
        lines.len() > 40,
        "expected the whole log, got {}",
        lines.len()
    );
    assert!(
        lines.iter().all(|line| line.contains(&reference)),
        "grep {reference} would miss {} of {} lines",
        lines.iter().filter(|l| !l.contains(&reference)).count(),
        lines.len()
    );
}

#[test]
fn the_body_names_a_command_that_finds_the_record() {
    let recorder = NotificationRecorder::new();
    let directory = tempfile::tempdir().expect("temporary log directory");
    let log = directory.path().join("engine.log");
    let reporter = FailureReporter::new(recorder.command().to_owned()).with_log_file(&log);
    let mut command = shell("printf 'boom\\n' >&2; exit 8");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let alert = recorder.wait();
    let reference = alert
        .split("Reference: ")
        .nth(1)
        .and_then(|rest| rest.split('.').next())
        .expect("body carries a reference")
        .to_owned();
    assert!(
        alert.contains(&format!("grep {reference} {}", log.display())),
        "the body must name where the record actually is: {alert}"
    );
}

#[test]
fn a_reporter_with_nowhere_to_send_the_user_omits_the_details_line() {
    // Pointing at a journal unit that never had the record is worse than
    // saying nothing: the user runs the command and learns nothing.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell("printf 'boom\\n' >&2; exit 8");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, CLEAN_EXIT);

    let alert = recorder.wait();
    assert!(!alert.contains("Details:"), "{alert}");
    assert!(alert.contains("status 8"), "{alert}");
}
