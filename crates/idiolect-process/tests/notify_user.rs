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
fn a_configured_command_receives_every_notification() {
    // The counterpart to the empty-command case: this one CAN fail, because
    // the recorder is actually wired in. Asserting that an empty command
    // produces nothing in a recorder that was never given that command is
    // unfalsifiable — no implementation could ever write to it.
    let recorder = NotificationRecorder::new();

    for body in ["first", "second"] {
        notify_user(recorder.command(), "Idiolect", body);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while recorder.records().len() < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "only {} of 2 notifications arrived",
            recorder.records().len()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
