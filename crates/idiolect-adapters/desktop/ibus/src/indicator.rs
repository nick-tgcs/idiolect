//! "Voice is live" indicator abstraction. While dictation is recording the
//! engine shows a small mic overlay next to the caret; this hides the concrete
//! GUI behind a trait (and, by default, a process boundary) so it is swappable
//! and its dependencies stay out of the IME.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

/// Shows the recording indicator at a caret position, repositions it while it's
/// already showing, and hides it. All calls are idempotent.
pub trait RecordingIndicator: Send + Sync {
    /// Show at, or move to, the given caret screen position.
    fn show(&self, x: i32, y: i32);
    fn hide(&self);
}

struct Running {
    child: Child,
    stdin: ChildStdin,
}

/// Launches an external overlay binary, streaming caret positions to its stdin
/// so it tracks the text caret, and kills it to hide. Keeping it out-of-process
/// means the overlay's GUI stack never runs inside the async IME.
pub struct SubprocessIndicator {
    binary: PathBuf,
    state: Mutex<Option<Running>>,
}

impl SubprocessIndicator {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            state: Mutex::new(None),
        }
    }

    /// Find the overlay binary next to the running engine binary, else by name.
    pub fn discover() -> Self {
        const NAME: &str = "idiolect-recording-indicator";
        let beside_engine = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)))
            .filter(|path| path.exists());
        Self::new(beside_engine.unwrap_or_else(|| PathBuf::from(NAME)))
    }
}

impl RecordingIndicator for SubprocessIndicator {
    fn show(&self, x: i32, y: i32) {
        let mut guard = self.state.lock().expect("indicator mutex");
        if let Some(running) = guard.as_mut() {
            // Already showing — stream the new caret position so it follows.
            let _ = writeln!(running.stdin, "{x} {y}");
            let _ = running.stdin.flush();
            return;
        }
        if let Ok(mut child) = Command::new(&self.binary)
            .arg(x.to_string())
            .arg(y.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            match child.stdin.take() {
                Some(stdin) => *guard = Some(Running { child, stdin }),
                None => {
                    let _ = child.kill();
                }
            }
        }
    }

    fn hide(&self) {
        if let Some(mut running) = self.state.lock().expect("indicator mutex").take() {
            let _ = running.child.kill();
            let _ = running.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_then_hide_a_short_lived_process() {
        // `cat` stands in for the overlay: it reads stdin (our position stream)
        // and stays alive until hide() kills it.
        let indicator = SubprocessIndicator::new("cat");
        indicator.show(30, 30);
        assert!(indicator.state.lock().unwrap().is_some(), "spawned");
        // Showing again repositions via stdin rather than respawning.
        indicator.show(40, 50);
        assert!(
            indicator.state.lock().unwrap().is_some(),
            "still one process"
        );
        indicator.hide();
        assert!(indicator.state.lock().unwrap().is_none(), "killed");
        indicator.hide(); // no-op
    }
}
