//! Contract for the notifier itself: `<command> <summary> <body>`, empty
//! disables it, and telling the user about a failure must never itself fail.

use idiolect_process::notify_user;
use idiolect_test_support::notifications::NotificationRecorder;

#[test]
fn invokes_the_command_with_summary_and_body() {
    let recorder = NotificationRecorder::new();

    notify_user(recorder.command(), "Idiolect", "translation unavailable");

    let line = recorder.wait();
    assert_eq!(
        line.lines().next(),
        Some("Idiolect|translation unavailable"),
        "summary and body arrive as the two positional args"
    );
}

#[test]
fn a_missing_binary_and_an_empty_command_are_silent_noops() {
    notify_user("/nonexistent/idiolect-notifier-xyz", "s", "b"); // must not panic
    notify_user("", "s", "b"); // disabled — must not panic or spawn
}

#[test]
fn an_empty_command_does_not_run_anything() {
    let recorder = NotificationRecorder::new();

    notify_user("", "Idiolect", "should never be delivered");

    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        recorder.records().is_empty(),
        "an empty notify command must disable notifications entirely"
    );
}
