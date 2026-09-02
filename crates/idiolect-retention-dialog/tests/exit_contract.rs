//! End-to-end contract for the SHIPPED binary, run as the daemon runs it.
//!
//! Everything else that asserts on these exit codes uses an `sh` stand-in that
//! exits by fiat, so both ends of the contract could drift with the whole suite
//! green. This runs the real thing.
//!
//! Only the "cannot start" path is exercised here, because it is the one that
//! is deterministic without a display. It is also the one that matters: it was
//! previously indistinguishable from a user cancelling.

use std::process::{Command, Stdio};

use idiolect_process::dialog::{CANCELLED_MARKER, EXIT_UNAVAILABLE};

#[test]
fn without_a_display_it_exits_unavailable_rather_than_looking_like_a_cancel() {
    // With `fn main() -> eframe::Result<()>` this exited 1 — the cancel code —
    // so a dialog that could not start looked exactly like the user declining,
    // and the daemon silently left the setting alone without a word.
    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-retention-dialog"))
        .arg("365")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .output()
        .expect("run the retention dialog");

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
    assert!(
        output.stdout.is_empty(),
        "a dialog that never ran must not claim a value: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn a_dialog_that_never_ran_does_not_emit_the_cancel_marker() {
    // The marker is what proves the user chose to cancel. If a failing dialog
    // could emit it, the caller would go back to silently discarding failures.
    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-retention-dialog"))
        .arg("365")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .output()
        .expect("run the retention dialog");

    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(CANCELLED_MARKER),
        "a failure claimed to be a cancellation"
    );
}
