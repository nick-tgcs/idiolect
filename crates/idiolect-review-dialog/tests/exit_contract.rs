//! End-to-end contract for the SHIPPED binary, run as the IBus engine runs it.
//!
//! Everything else that asserts on these exit codes uses an `sh` stand-in that
//! exits by fiat, so both ends of the contract could drift with the whole suite
//! green. This runs the real thing.
//!
//! Only the "cannot start" path is exercised here, because it is the one that
//! is deterministic without a display — and it is the one that costs the most:
//! the engine maps a cancel onto discarding the take, so a failure mistaken for
//! a cancel throws away every word the user just dictated.

use std::io::Write;
use std::process::{Command, Stdio};

use idiolect_process::dialog::{CANCELLED_MARKER, EXIT_UNAVAILABLE};

#[test]
fn without_a_display_it_exits_unavailable_rather_than_looking_like_a_cancel() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_idiolect-review-dialog"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run the review dialog");
    // Deliver a take exactly as the engine does, then close the pipe.
    {
        let mut stdin = child.stdin.take().expect("stdin");
        let _ = writeln!(stdin, "final the whole dictated take");
    }
    let output = child
        .wait_with_output()
        .expect("wait for the review dialog");

    assert_eq!(
        output.status.code(),
        Some(EXIT_UNAVAILABLE),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.stderr.is_empty(),
        "an unavailable dialog must say why"
    );
}

#[test]
fn a_dialog_that_never_ran_does_not_emit_the_cancel_marker() {
    // The marker is what proves the user chose to cancel. If a failing dialog
    // could emit it, the engine would go straight back to discarding takes in
    // silence — the exact bug this contract exists to close.
    let mut child = Command::new(env!("CARGO_BIN_EXE_idiolect-review-dialog"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run the review dialog");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        let _ = writeln!(stdin, "final the whole dictated take");
    }
    let output = child
        .wait_with_output()
        .expect("wait for the review dialog");

    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(CANCELLED_MARKER),
        "a failure claimed to be a cancellation"
    );
}
