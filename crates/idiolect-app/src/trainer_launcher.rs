//! Spawns `idiolect-trainerctl train` as a subprocess and streams progress
//! back to the dashboard. Used by `LocalBackend` in standalone mode.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use crate::model::TrainingProgress;

/// All the arguments needed to launch a training run.
#[derive(Clone)]
pub(crate) struct TrainerConfig {
    pub(crate) db_path: PathBuf,
    pub(crate) audio_root: PathBuf,
    pub(crate) base_model: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) serve: Option<PathBuf>,
    pub(crate) gpu: bool,
}

/// A running trainer subprocess. Dropped when training completes or is
/// abandoned; the subprocess is reaped by the background thread.
pub(crate) struct TrainerLauncher {
    progress_rx: Receiver<TrainingProgress>,
    done_rx: Receiver<Result<(), String>>,
}

/// Returned by [`TrainerLauncher::poll`] each tick.
pub(crate) struct PollResult {
    /// Latest progress line, if one arrived since the last call.
    pub(crate) progress: Option<TrainingProgress>,
    /// `Some(Ok(()))` on success, `Some(Err(_))` on failure, `None` = still running.
    pub(crate) done: Option<Result<(), String>>,
}

impl TrainerLauncher {
    /// Spawn `idiolect-trainerctl train …` and return immediately. Progress and
    /// completion are delivered via [`poll`].
    pub(crate) fn start(cfg: &TrainerConfig) -> Result<Self, std::io::Error> {
        let mut cmd = Command::new("idiolect-trainerctl");
        cmd.arg("train")
            .arg("--db")
            .arg(&cfg.db_path)
            .arg("--audio-root")
            .arg(&cfg.audio_root)
            .arg("--base-model")
            .arg(&cfg.base_model)
            .arg("--output")
            .arg(&cfg.output);
        if let Some(serve) = &cfg.serve {
            cmd.arg("--serve").arg(serve);
        }
        if cfg.gpu {
            cmd.arg("--gpu");
        }
        let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;

        let stderr = child.stderr.take().expect("stderr piped");
        let stdout = child.stdout.take().expect("stdout piped");

        let (progress_tx, progress_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

        // Parse stderr progress lines on a background thread.
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(p) = parse_progress_line(&line) {
                    let _ = progress_tx.send(p);
                }
            }
        });

        // Collect the stdout JSON report and reap the child.
        std::thread::spawn(move || {
            use std::io::Read;
            let mut report_json = String::new();
            BufReader::new(stdout).read_to_string(&mut report_json).ok();
            let _ = child.wait(); // reap — non-blocking once stdout closed
            let result = if report_json.contains(r#""output""#) {
                Ok(())
            } else {
                Err(format!("unexpected trainer output: {report_json}"))
            };
            let _ = done_tx.send(result);
        });

        Ok(Self {
            progress_rx,
            done_rx,
        })
    }

    /// Test-only: a launcher wired to caller-held channels instead of a subprocess.
    /// It reports "still running" until something is sent on the done sender.
    #[cfg(test)]
    pub(crate) fn test_stub() -> (
        Self,
        mpsc::Sender<TrainingProgress>,
        mpsc::Sender<Result<(), String>>,
    ) {
        let (progress_tx, progress_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        (
            Self {
                progress_rx,
                done_rx,
            },
            progress_tx,
            done_tx,
        )
    }

    /// Non-blocking poll. Drains to the latest progress update.
    pub(crate) fn poll(&self) -> PollResult {
        let mut progress = None;
        while let Ok(p) = self.progress_rx.try_recv() {
            progress = Some(p);
        }
        let done = self.done_rx.try_recv().ok();
        PollResult { progress, done }
    }
}

/// Parse a `"epoch E/T sample S/Total loss L"` stderr line into a
/// [`TrainingProgress`]. Returns `None` for unknown / summary lines.
pub(crate) fn parse_progress_line(line: &str) -> Option<TrainingProgress> {
    // Expected: "epoch 1/2 sample 45/120 loss 0.3350"
    // Ignored:  "prepared N example(s)"
    // Ignored:  "epoch E/T mean loss M"  (no "sample" keyword)
    let line = line.trim();
    if !line.starts_with("epoch ") || !line.contains(" sample ") {
        return None;
    }
    let mut parts = line.split_whitespace();
    let _epoch_kw = parts.next()?; // "epoch"
    let epoch_frac = parts.next()?; // "1/2"
    let _sample_kw = parts.next()?; // "sample"
    let sample_frac = parts.next()?; // "45/120"
    let _loss_kw = parts.next()?; // "loss"
    let loss_str = parts.next()?; // "0.3350"

    let (epoch, epochs) = parse_fraction(epoch_frac)?;
    let (sample, total) = parse_fraction(sample_frac)?;
    let loss_now: f32 = loss_str.parse().ok()?;

    Some(TrainingProgress {
        epoch,
        epochs,
        sample,
        total,
        loss_before: 0.0,
        loss_now,
    })
}

fn parse_fraction(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('/')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_epoch_progress_line_is_parsed() {
        let p = parse_progress_line("epoch 1/2 sample 45/120 loss 0.3350").unwrap();
        assert_eq!(p.epoch, 1);
        assert_eq!(p.epochs, 2);
        assert_eq!(p.sample, 45);
        assert_eq!(p.total, 120);
        assert!((p.loss_now - 0.335_0).abs() < 1e-4);
        assert_eq!(p.loss_before, 0.0);
    }

    #[test]
    fn final_sample_of_last_epoch_is_parsed() {
        let p = parse_progress_line("epoch 2/2 sample 120/120 loss 0.1901").unwrap();
        assert_eq!(p.epoch, 2);
        assert_eq!(p.epochs, 2);
        assert_eq!(p.sample, 120);
        assert_eq!(p.total, 120);
        assert!((p.loss_now - 0.1901).abs() < 1e-4);
    }

    #[test]
    fn first_sample_of_first_epoch_is_parsed() {
        let p = parse_progress_line("epoch 1/2 sample 1/2 loss 5.1234").unwrap();
        assert_eq!(p.epoch, 1);
        assert_eq!(p.sample, 1);
        assert!((p.loss_now - 5.1234).abs() < 1e-4);
    }

    #[test]
    fn prepared_line_is_ignored() {
        assert!(parse_progress_line("prepared 5 example(s)").is_none());
    }

    #[test]
    fn epoch_summary_line_is_ignored() {
        assert!(parse_progress_line("epoch 2/2 mean loss 0.2345").is_none());
    }

    #[test]
    fn empty_line_is_ignored() {
        assert!(parse_progress_line("").is_none());
    }

    #[test]
    fn garbage_line_is_ignored() {
        assert!(parse_progress_line("some random output from the trainer").is_none());
    }
}
