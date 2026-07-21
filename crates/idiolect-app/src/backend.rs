//! The [`Backend`] trait bridges the dashboard view from the state source and
//! action sink. Two implementations exist:
//!
//! * [`PipeBackend`] — attached mode (Linux): reads JSON snapshots from stdin,
//!   writes action-ids to stdout. The daemon drives both ends.
//! * [`LocalBackend`] — standalone mode (macOS / Windows): owns a [`SyncHost`]
//!   and a `TrainerLauncher` in-process; the daemon is not involved.

use crate::model::Snapshot;

/// The seam between the egui view and whatever manages state.
pub(crate) trait Backend: Send + 'static {
    /// Return the most recent snapshot, or `None` if no new one has arrived
    /// since the last call.
    fn poll_state(&mut self) -> Option<Snapshot>;
    /// Send an action-id string to the host.
    fn send(&mut self, action: &str);
}

// ── PipeBackend (attached mode: Linux daemon drives this via subprocess) ──────

use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// Reads newline-delimited JSON snapshots from stdin on a background thread and
/// writes action-ids to stdout. Used when the daemon spawns `idiolect-app` as a
/// subprocess (Linux / attached mode).
pub(crate) struct PipeBackend {
    rx: Receiver<Snapshot>,
    stdout: std::io::Stdout,
}

impl PipeBackend {
    /// Spawn the stdin reader thread and return the backend. Panics if stdin is
    /// not connected (which never happens in the subprocess deployment).
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let stdin = std::io::stdin();
            let reader = BufReader::new(stdin.lock());
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match Snapshot::from_json(&line) {
                    Ok(snap) => {
                        if tx.send(snap).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        eprintln!("idiolect-app: bad snapshot JSON: {err}");
                    }
                }
            }
        });
        Self {
            rx,
            stdout: std::io::stdout(),
        }
    }
}

impl Default for PipeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for PipeBackend {
    fn poll_state(&mut self) -> Option<Snapshot> {
        // Drain to the latest; discard stale snapshots.
        let mut latest = None;
        loop {
            match self.rx.try_recv() {
                Ok(snap) => latest = Some(snap),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        latest
    }

    fn send(&mut self, action: &str) {
        let _ = writeln!(self.stdout, "{action}");
        let _ = self.stdout.flush();
    }
}
