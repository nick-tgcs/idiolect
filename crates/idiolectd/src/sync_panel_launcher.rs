//! Out-of-process Corrections Dashboard. The dashboard shows phone-sync status,
//! pairing QR, and training controls.
//!
//! Subprocess contract (the daemon side of `idiolect-app`):
//!   args   : `--standalone` so the app owns the sync server in-process (the
//!            daemon runs none — see docs/future/010 for the deferred
//!            daemon-embedded mode).
//!   env    : `IDIOLECT_DATA_DIR` / `IDIOLECT_DB_PATH` / `IDIOLECT_BASE_MODEL`
//!            — the daemon's resolved store, so the dashboard pairs phones and
//!            trains against the database the daemon actually writes, never a
//!            parallel default-path store. `IDIOLECT_HISTORY_KEY` (only when
//!            the daemon encrypts history) — the at-rest key file, so the
//!            dashboard's sync ingest encrypts `ime_text_history` rows the
//!            same way the daemon does instead of writing plaintext into an
//!            encrypted database.
//!   stdout : forwarded line-by-line as tray actions. The standalone dashboard
//!            currently emits none (it handles its own actions in-process);
//!            the reader doubles as the exit reaper that re-arms the launcher.
//!   exit   : whenever the user closes the window.
//!
//! Like every GUI here, it is best-effort: a missing or crashing binary must
//! never affect dictation.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use crate::observed_child::{FailureReporter, ObservedChild};
use idiolect_adapter_ksni::TrayCallback;
use idiolect_common::config::dashboard_store_env;

/// The daemon-resolved store the dashboard must operate on, passed through the
/// child's environment ([`dashboard_store_env`] — the same constants
/// `idiolect-app`'s `main.rs` reads, so the names cannot drift apart).
pub(crate) struct DashboardStore {
    /// Root for the sync-owned artifacts the dashboard derives itself
    /// (device tokens, served/personal model slots, audio).
    pub(crate) data_dir: PathBuf,
    /// The daemon's metadata database — `db/idiolect.sqlite` under the data
    /// root by default, NOT the dashboard's own `idiolect.db` convention.
    pub(crate) database_path: PathBuf,
    /// The daemon's active ASR model: the trainer's merge base and the served
    /// slot's first-run seed.
    pub(crate) base_model: PathBuf,
    /// The daemon's at-rest history key file — `Some` only when the daemon
    /// encrypts history (`[history] encrypt_at_rest`). The dashboard's sync
    /// ingest writes `ime_text_history` rows through `commit_session`; without
    /// the key those land as PLAINTEXT in the daemon's encrypted database.
    pub(crate) history_key: Option<PathBuf>,
}

pub(crate) struct SyncPanelLauncher {
    binary: PathBuf,
    args: Vec<String>,
    store: Option<DashboardStore>,
    reporter: FailureReporter,
    window_open: Arc<AtomicBool>,
}

impl SyncPanelLauncher {
    #[cfg(test)]
    pub(crate) fn with_command(binary: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self::configured(binary, args, None, String::new())
    }

    #[cfg(test)]
    pub(crate) fn with_command_and_store(
        binary: impl Into<PathBuf>,
        args: Vec<String>,
        store: DashboardStore,
    ) -> Self {
        Self::configured(binary, args, Some(store), String::new())
    }

    fn configured(
        binary: impl Into<PathBuf>,
        args: Vec<String>,
        store: Option<DashboardStore>,
        notify_command: impl Into<String>,
    ) -> Self {
        Self {
            binary: binary.into(),
            args,
            store,
            reporter: FailureReporter::new(notify_command),
            window_open: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Find the dashboard binary next to the running daemon binary, falling
    /// back to its plain name (resolved via `PATH`); every launch carries the
    /// daemon's [`DashboardStore`].
    pub(crate) fn discover(store: DashboardStore, notify_command: &str) -> Self {
        const NAME: &str = "idiolect-app";
        let beside_daemon = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)))
            .filter(|path| path.exists());
        Self::configured(
            beside_daemon.unwrap_or_else(|| PathBuf::from(NAME)),
            Vec::new(),
            Some(store),
            notify_command,
        )
    }

    /// Open the dashboard. Returns immediately; the window is serviced by a
    /// dedicated thread that also reaps the process. A second call while the
    /// dashboard is open is a no-op.
    pub(crate) fn open(&self, forward: mpsc::Sender<TrayCallback>) {
        if self.window_open.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut command = Command::new(&self.binary);
        command
            .args(&self.args)
            .arg("--standalone")
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        if let Some(store) = &self.store {
            command
                .env(dashboard_store_env::DATA_DIR, &store.data_dir)
                .env(dashboard_store_env::DB_PATH, &store.database_path)
                .env(dashboard_store_env::BASE_MODEL, &store.base_model);
            // Set only when the daemon encrypts: absence tells the app "no
            // cipher". The remove comes first because the child otherwise
            // INHERITS the daemon's own environment — a stale
            // IDIOLECT_HISTORY_KEY there (wrapper script, systemd Environment=)
            // would make the dashboard cipher a store this daemon reads
            // plaintext.
            command.env_remove(dashboard_store_env::HISTORY_KEY);
            if let Some(history_key) = &store.history_key {
                command.env(dashboard_store_env::HISTORY_KEY, history_key);
            }
        }
        let Some(mut child) =
            ObservedChild::spawn(&mut command, "Corrections Dashboard", self.reporter.clone())
        else {
            self.window_open.store(false, Ordering::SeqCst);
            return;
        };
        let stdout = child.child_mut().stdout.take();
        let window_open = Arc::clone(&self.window_open);
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                for line in std::io::BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.is_empty() {
                        continue;
                    }
                    if forward.send(TrayCallback::Activate(line)).is_err() {
                        break;
                    }
                }
            }
            let _ = child.wait(&[0]);
            window_open.store(false, Ordering::SeqCst);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn script_launcher(script: &str) -> SyncPanelLauncher {
        SyncPanelLauncher::with_command("sh", vec!["-c".to_owned(), script.to_owned()])
    }

    #[test]
    fn forwards_dashboard_actions_as_tray_activations() {
        let launcher = script_launcher(r#"printf 'train:now\nsync:pair\n'"#);
        let (tx, rx) = mpsc::channel();
        launcher.open(tx);

        let TrayCallback::Activate(first) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first action");
        assert_eq!(first, "train:now");
        let TrayCallback::Activate(second) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("second action");
        assert_eq!(second, "sync:pair");
    }

    #[test]
    fn only_one_window_at_a_time_and_reopens_after_exit() {
        let launcher = script_launcher(
            r#"printf 'spawned\n'
               sleep 0.4"#,
        );
        let (tx, rx) = mpsc::channel();
        launcher.open(tx.clone());
        launcher.open(tx.clone()); // ignored: still open
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "first spawn"
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(700)).is_err(),
            "no second window while the first is open"
        );
        launcher.open(tx);
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "reopens once the previous window closed"
        );
    }

    #[test]
    fn the_daemon_store_reaches_the_dashboard_environment() {
        // The dashboard runs standalone but must operate on the DAEMON's store
        // (its database lives at db/idiolect.sqlite, not the dashboard's own
        // idiolect.db default — a mismatch silently splits corrections and
        // training across two stores). The launcher hands the resolved paths
        // over in the environment; the child echoes them back over the same
        // stdout channel the action forwarding uses, so this is pinned end to
        // end against a real subprocess.
        let launcher = SyncPanelLauncher::with_command_and_store(
            "sh",
            vec![
                "-c".to_owned(),
                r#"printf '%s\n%s\n%s\n%s\n' "${IDIOLECT_DATA_DIR:-unset}" \
                   "${IDIOLECT_DB_PATH:-unset}" "${IDIOLECT_BASE_MODEL:-unset}" \
                   "${IDIOLECT_HISTORY_KEY:-unset}""#
                    .to_owned(),
            ],
            DashboardStore {
                data_dir: "/data/root".into(),
                database_path: "/data/root/db/idiolect.sqlite".into(),
                base_model: "/data/root/models/whisper/ggml-base.en.bin".into(),
                history_key: Some("/data/root/db/history.key".into()),
            },
        );
        let (tx, rx) = mpsc::channel();
        launcher.open(tx);

        let mut lines = Vec::new();
        for _ in 0..4 {
            let TrayCallback::Activate(line) =
                rx.recv_timeout(Duration::from_secs(5)).expect("env line");
            lines.push(line);
        }
        assert_eq!(
            lines,
            vec![
                "/data/root".to_owned(),
                "/data/root/db/idiolect.sqlite".to_owned(),
                "/data/root/models/whisper/ggml-base.en.bin".to_owned(),
                "/data/root/db/history.key".to_owned(),
            ]
        );
    }

    #[test]
    fn an_unencrypted_daemon_leaves_the_history_key_unset() {
        // Absence IS the contract: the app treats an unset/empty
        // IDIOLECT_HISTORY_KEY as "no cipher", so a daemon that does not
        // encrypt must not export the variable at all — exporting a dummy
        // value would make the dashboard encrypt a store the daemon reads
        // plaintext.
        let launcher = SyncPanelLauncher::with_command_and_store(
            "sh",
            vec![
                "-c".to_owned(),
                r#"printf '%s\n' "${IDIOLECT_HISTORY_KEY:-unset}""#.to_owned(),
            ],
            DashboardStore {
                data_dir: "/data/root".into(),
                database_path: "/data/root/db/idiolect.sqlite".into(),
                base_model: "/data/root/models/whisper/ggml-base.en.bin".into(),
                history_key: None,
            },
        );
        let (tx, rx) = mpsc::channel();
        launcher.open(tx);

        let TrayCallback::Activate(line) =
            rx.recv_timeout(Duration::from_secs(5)).expect("env line");
        assert_eq!(line, "unset");
    }

    #[test]
    fn a_missing_binary_is_a_safe_noop_that_does_not_wedge_the_launcher() {
        let launcher = SyncPanelLauncher::with_command("/nonexistent/idiolect-app-xyz", Vec::new());
        let (tx, rx) = mpsc::channel();
        launcher.open(tx.clone());
        launcher.open(tx);
        assert!(rx.try_recv().is_err(), "nothing forwarded");
        assert!(
            !launcher.window_open.load(Ordering::SeqCst),
            "failed spawn releases the open flag"
        );
    }
}
