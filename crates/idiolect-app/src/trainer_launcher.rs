//! Spawns `idiolect-trainerctl train` as a subprocess and streams progress
//! back to the dashboard. Used by `LocalBackend` in standalone mode.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
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

/// Resolve the trainer binary: prefer the `idiolect-trainerctl` shipped beside the
/// running app (the desktop archive lays the two out side by side, and a GUI launch
/// does not put that directory on `PATH`), falling back to its plain name (resolved
/// via `PATH`).
fn trainerctl_program(app_exe: Option<PathBuf>) -> PathBuf {
    let name = format!("idiolect-trainerctl{}", std::env::consts::EXE_SUFFIX);
    app_exe
        .as_deref()
        .and_then(Path::parent)
        .map(|dir| dir.join(&name))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

impl TrainerLauncher {
    /// Spawn `idiolect-trainerctl train …` and return immediately. Progress and
    /// completion are delivered via [`poll`].
    pub(crate) fn start(cfg: &TrainerConfig) -> Result<Self, std::io::Error> {
        let mut cmd = Command::new(trainerctl_program(std::env::current_exe().ok()));
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

/// Installs a fake `idiolect-trainerctl` BESIDE the running test binary — the layout
/// the desktop archive ships — so spawn-path tests can prove `start` finds the
/// sibling. One shared mutex, because every test shares that one on-disk path.
#[cfg(all(test, unix))]
pub(crate) mod test_sibling {
    use std::path::PathBuf;
    use std::sync::Mutex;

    static SIBLING: Mutex<()> = Mutex::new(());

    /// Write `script` as an executable `idiolect-trainerctl` next to the current
    /// test binary, run `body`, then remove it — serialized across tests.
    pub(crate) fn with_fake_trainerctl<R>(script: &str, body: impl FnOnce() -> R) -> R {
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _guard = SIBLING.lock().unwrap_or_else(|e| e.into_inner());
        let path = std::env::current_exe()
            .expect("current_exe")
            .parent()
            .expect("test binary has a parent dir")
            .join("idiolect-trainerctl");
        std::fs::write(&path, script).expect("write fake trainerctl");
        let _cleanup = Cleanup(path.clone());
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).expect("stat").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake trainerctl");
        body()
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

    #[test]
    fn the_trainerctl_beside_the_app_is_preferred() {
        let dir = tempfile::tempdir().expect("tempdir");
        let name = format!("idiolect-trainerctl{}", std::env::consts::EXE_SUFFIX);
        let sibling = dir.path().join(&name);
        std::fs::write(&sibling, b"").expect("create sibling");

        let program = trainerctl_program(Some(dir.path().join("idiolect-app")));

        assert_eq!(
            program, sibling,
            "the packaged sibling must win over a PATH lookup"
        );
    }

    #[test]
    fn a_missing_sibling_falls_back_to_a_path_lookup() {
        let dir = tempfile::tempdir().expect("tempdir");

        let program = trainerctl_program(Some(dir.path().join("idiolect-app")));

        assert_eq!(
            program,
            PathBuf::from(format!(
                "idiolect-trainerctl{}",
                std::env::consts::EXE_SUFFIX
            )),
            "with nothing shipped beside the app, the plain name keeps PATH working"
        );
    }

    #[test]
    fn a_directory_named_like_the_trainer_falls_back_to_a_path_lookup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let name = format!("idiolect-trainerctl{}", std::env::consts::EXE_SUFFIX);
        std::fs::create_dir(dir.path().join(&name)).expect("create decoy dir");

        let program = trainerctl_program(Some(dir.path().join("idiolect-app")));

        assert_eq!(
            program,
            PathBuf::from(&name),
            "a non-file dirent beside the app must not shadow a real PATH install"
        );
    }

    #[test]
    fn an_unknown_app_location_falls_back_to_a_path_lookup() {
        let program = trainerctl_program(None);

        assert_eq!(
            program,
            PathBuf::from(format!(
                "idiolect-trainerctl{}",
                std::env::consts::EXE_SUFFIX
            )),
            "a failed current_exe lookup must not lose training entirely"
        );
    }

    /// Poll until the trainer reports completion (or give up after ten seconds).
    #[cfg(unix)]
    fn wait_done(launcher: &TrainerLauncher) -> Result<(), String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(done) = launcher.poll().done {
                return done;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "trainer did not finish in time"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    // unix-only: the fake trainerctl is a shell script, which Windows cannot exec.
    // The resolution logic itself is covered cross-platform by the unit tests above.
    #[cfg(unix)]
    #[test]
    fn start_spawns_the_trainerctl_shipped_beside_the_app() {
        // The desktop archive ships `idiolect-trainerctl` NEXT TO the app binary,
        // and a GUI launch does not put that directory on PATH — so `start` must
        // prefer the sibling over a PATH lookup.
        let script = "#!/bin/sh\nprintf '{\"output\":\"ok\"}'\n";
        test_sibling::with_fake_trainerctl(script, || {
            let dir = tempfile::tempdir().expect("tempdir");
            let cfg = TrainerConfig {
                db_path: dir.path().join("db.sqlite"),
                audio_root: dir.path().join("audio"),
                base_model: dir.path().join("base.bin"),
                output: dir.path().join("out.bin"),
                serve: None,
                gpu: false,
            };
            let launcher =
                TrainerLauncher::start(&cfg).expect("spawn the trainerctl beside the app");
            assert_eq!(
                wait_done(&launcher),
                Ok(()),
                "the sibling trainerctl must run to completion"
            );
        });
    }
}
