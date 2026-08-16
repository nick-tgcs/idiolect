//! Contract for helper-process supervision, exercised through the public API
//! with real child processes — the failure modes here (a full pipe, a signal,
//! a descendant holding a descriptor) do not exist for an in-process fake.
//!
//! Coverage levels for this behaviour, per CONTRIBUTING.md:
//!   * unit — the launchers' own modules in `idiolectd`, plus
//!     `every_helper_launcher_reports_through_the_configured_notify_command`
//!     in `run_loop`, which pins the one line joining config to launchers.
//!   * integration — this file: real processes, real pipes, real signals, and
//!     a real notifier binary.
//!   * e2e — NOT REACHABLE headlessly, deliberately. All three helper launches
//!     are triggered only by a D-Bus tray activation
//!     (`run_loop::handle_tray_callback`); CI runs with `IDIOLECT_DISABLE_TRAY=1`
//!     and no StatusNotifierWatcher, and the `idiolect tray` CLI reads and
//!     writes the settings store directly rather than dispatching an action, so
//!     nothing outside the process can ask the daemon to open a helper.

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use idiolect_process::{FailureReporter, ObservedChild};
use idiolect_test_support::notifications::NotificationRecorder;

/// Long enough for a loaded CI runner under coverage instrumentation, short
/// enough that a genuine hang fails the test instead of the suite timeout.
const WAIT_BUDGET: Duration = Duration::from_secs(30);

fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", script]).stdin(Stdio::null());
    command
}

/// Runs `wait` off-thread so a supervision bug fails as a timeout with a
/// message, rather than hanging the whole test binary.
fn wait_within_budget(child: ObservedChild, expected: &[i32]) {
    let expected = expected.to_vec();
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = child.wait(&expected);
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
    wait_within_budget(child, &[0, 1]);

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
    wait_within_budget(child, &[0]);

    let alert = recorder.wait();
    assert!(alert.contains("Idiolect Settings failed"), "{alert}");
    assert!(alert.contains("status 23"), "{alert}");
    assert!(alert.contains("no GPU adapter found"), "{alert}");
}

#[test]
fn a_shutdown_signal_is_silent_but_a_crash_signal_is_reported() {
    // systemd's default KillMode=control-group SIGTERMs open helper windows
    // when the daemon restarts; that is not the helper failing.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut terminated = shell("kill -TERM $$; sleep 5");

    let child = ObservedChild::spawn(&mut terminated, "Settings", reporter.clone()).expect("spawn");
    wait_within_budget(child, &[0]);

    std::thread::sleep(Duration::from_millis(200));
    assert!(
        recorder.records().is_empty(),
        "a daemon restart alerted the user: {:?}",
        recorder.records()
    );

    let mut aborted = shell("kill -ABRT $$; sleep 5");
    let child = ObservedChild::spawn(&mut aborted, "Settings", reporter).expect("spawn");
    wait_within_budget(child, &[0]);

    let alert = recorder.wait();
    assert!(alert.contains("terminated by signal 6"), "{alert}");
}

#[test]
fn a_chatty_helper_runs_to_completion_because_stderr_is_drained_past_the_cap() {
    // Only the RETAINED prefix is capped at 16 KiB; the reader must keep
    // consuming. If it stopped at the cap it would drop the pipe, and the
    // helper's next write would take SIGPIPE — killing the user's window
    // mid-work, not merely truncating a log. So the pin is that the helper
    // still reaches its final action, not that we captured a nice string.
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
    wait_within_budget(child, &[0]);

    assert!(
        marker.exists(),
        "the helper died before finishing — its stderr was not drained"
    );
    let alert = recorder.wait();
    assert!(alert.contains("stderr truncated"), "{alert}");
}

#[test]
fn the_notification_keeps_the_end_of_stderr_where_the_reason_is() {
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell(
        "i=0; while [ $i -lt 200 ]; do printf 'libEGL warning: DRI3 probe failed\\n' >&2; \
         i=$((i+1)); done; printf 'no GPU adapter found - install mesa-vulkan-drivers\\n' >&2; \
         exit 4",
    );

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, &[0]);

    let alert = recorder.wait();
    assert!(
        alert.contains("install mesa-vulkan-drivers"),
        "the actionable last line was truncated away: {alert}"
    );
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
    wait_within_budget(child, &[0]);

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
fn a_nul_byte_in_stderr_still_produces_a_notification() {
    // `Command::arg` rejects a NUL, and the notifier swallows spawn errors —
    // so an unsanitised NUL loses the alert entirely, and silently.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell("printf 'boom\\000more\\n' >&2; exit 7");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, &[0]);

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
        .wait_with_diagnostic(&[0, 1], Some("could not read dialog output: broken pipe"))
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
fn a_descendant_holding_stderr_cannot_wedge_the_caller() {
    // Before the drain was bounded, joining the reader meant waiting for every
    // holder of the pipe — so a stray grandchild pinned the launcher's
    // "window open" latch and the tray item never reopened.
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    // The helper exits at once; the backgrounded sleep inherits stderr.
    let mut command = shell("sleep 60 & exit 0");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    let started = std::time::Instant::now();
    wait_within_budget(child, &[0]);

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "wait() waited for a descendant, not the child: {:?}",
        started.elapsed()
    );
}

#[test]
fn an_identical_failure_does_not_toast_the_user_over_and_over() {
    // One broken thing fails on EVERY attempt: a helper missing from a partial
    // install fails on every take, and the engine launches its helpers per
    // take. A toast each time is how a user learns to dismiss Idiolect's
    // notifications without reading them.
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
fn a_different_failure_is_still_reported_while_another_is_suppressed() {
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());

    let mut missing = Command::new("/nonexistent/idiolect-helper-xyz");
    assert!(ObservedChild::spawn(&mut missing, "Settings", reporter.clone()).is_none());
    let mut crashing = shell("printf 'no GPU adapter\\n' >&2; exit 4");
    let child = ObservedChild::spawn(&mut crashing, "Dashboard", reporter).expect("spawn");
    wait_within_budget(child, &[0]);

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while recorder.records().len() < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "a distinct failure was suppressed: {:?}",
            recorder.records()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
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
fn an_empty_notify_command_disables_notifications_without_failing() {
    let reporter = FailureReporter::new(String::new());
    let mut command = shell("printf 'boom\\n' >&2; exit 3");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");

    // Must not panic and must still return the status.
    wait_within_budget(child, &[0]);
}

#[test]
fn the_notification_tells_the_user_how_to_find_the_matching_journal_lines() {
    let recorder = NotificationRecorder::new();
    let reporter = FailureReporter::new(recorder.command().to_owned());
    let mut command = shell("printf 'boom\\n' >&2; exit 8");

    let child = ObservedChild::spawn(&mut command, "Settings", reporter).expect("spawn");
    wait_within_budget(child, &[0]);

    let alert = recorder.wait();
    let reference = alert
        .split("Reference: ")
        .nth(1)
        .and_then(|rest| rest.split('.').next())
        .expect("body carries a reference")
        .to_owned();
    assert!(
        alert.contains(&format!("grep {reference}")),
        "the body must name a command that actually finds the reference: {alert}"
    );
}
