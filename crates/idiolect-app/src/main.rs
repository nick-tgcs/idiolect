//! Idiolect dashboard: phone↔PC sync status, pairing QR, and training controls.
//!
//! Subprocess contract (attached / Linux mode, daemon's `SyncPanelLauncher` drives this):
//!   stdin  : newline-delimited JSON state snapshots (see `model::Snapshot`)
//!   stdout : one action-id per line (`sync:pair`, `train:now`, …)
//!
//! Standalone mode: the app owns its own `SyncHost`. On macOS / Windows that is
//! a direct launch with the default store; the Linux daemon's tray also spawns
//! this mode (`--standalone`), handing over ITS store through the environment
//! (see [`standalone_store`]) so the dashboard acts on the database the daemon
//! writes.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use eframe::egui;
use idiolect_common::config::dashboard_store_env;

mod backend;
mod backend_local;
mod model;
mod sync_host;
mod theme;
mod trainer_launcher;
mod view;

use backend::Backend;
use model::DashboardModel;

fn main() -> eframe::Result<()> {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    // `--standalone` forces standalone (SyncHost) mode even when stdin is a pipe.
    // The Linux daemon uses this to open the dashboard without state piping.
    if std::env::args().any(|a| a == "--standalone") {
        std::env::set_var("IDIOLECT_STANDALONE", "1");
    }
    let backend = make_backend(rt.handle());
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("idiolect")
            .with_inner_size([420.0, 520.0])
            .with_resizable(true),
        ..Default::default()
    };
    eframe::run_native(
        "idiolect",
        native_options,
        Box::new(move |_cc| Ok(Box::new(DashboardApp::new_with(backend, rt)))),
    )
}

/// Discovers the machine's LAN-facing IP address by connecting a UDP socket
/// to an external address (no data is sent). Returns `None` if no route is
/// available (e.g. no network interface).
fn local_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

/// Returns the data directory for standalone mode.
fn data_dir() -> PathBuf {
    env_path(dashboard_store_env::DATA_DIR)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/idiolect"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/idiolect"))
}

/// The store a standalone launch operates on. Everything nests under [`data`]
/// except where the Linux daemon's tray launcher overrode a path through the
/// environment: the daemon keeps its database at `db/idiolect.sqlite` and its
/// ASR model under `models/whisper/`, so without the overrides the dashboard
/// would silently pair phones and train against a second, default-path store
/// the daemon never writes (env names mirrored in `idiolectd`'s
/// `sync_panel_launcher.rs`).
struct StandaloneStore {
    data: PathBuf,
    db_path: PathBuf,
    base_model: PathBuf,
    /// The daemon's at-rest history key, when the spawning daemon encrypts —
    /// sync ingest must cipher `ime_text_history` rows the way that daemon
    /// does. `None` (every direct launch) keeps the store plaintext.
    history_key: Option<PathBuf>,
}

/// Pure resolution: `db_env`/`base_env`/`history_key_env` are
/// [`dashboard_store_env::DB_PATH`] / [`dashboard_store_env::BASE_MODEL`] /
/// [`dashboard_store_env::HISTORY_KEY`] when set, and the fallbacks are the
/// historical single-directory plaintext layout of a direct standalone launch
/// (macOS / Windows).
fn standalone_store(
    data: PathBuf,
    db_env: Option<PathBuf>,
    base_env: Option<PathBuf>,
    history_key_env: Option<PathBuf>,
) -> StandaloneStore {
    let db_path = db_env.unwrap_or_else(|| data.join("idiolect.db"));
    let base_model = base_env.unwrap_or_else(|| data.join("ggml-base.en.bin"));
    StandaloneStore {
        data,
        db_path,
        base_model,
        history_key: history_key_env,
    }
}

/// A path from the environment; unset and empty both mean "not provided".
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The sync-host configuration of a standalone launch, derived from the
/// resolved [`StandaloneStore`]. Extracted so a test pins that the store's
/// fields (database, history key) actually reach the host —
/// [`make_backend`] itself is a process boundary.
fn standalone_sync_cfg(store: &StandaloneStore, pair_url: String) -> sync_host::SyncHostConfig {
    sync_host::SyncHostConfig {
        bind: "0.0.0.0:8765".parse().expect("valid addr"),
        pair_url,
        tls: false,
        db_path: store.db_path.clone(),
        audio_root: store.data.join("audio"),
        // One path, two roles: the trainer publishes here (`--serve`) and the
        // sync host serves the same file to paired phones (`/v1/model`).
        model_path: store.data.join("model.bin"),
        tokens_path: store.data.join("device_tokens.json"),
        history_key: store.history_key.clone(),
    }
}

/// Until the first training run publishes a personal model, copy the base
/// model into the served slot (when present) so a freshly paired phone can
/// complete onboarding instead of hitting 404. Staged copy + rename, so a
/// crash mid-copy can't leave a torn file where the host will serve it.
fn seed_served_model(base: &Path, served: &Path) {
    if served.exists() || !base.is_file() {
        return;
    }
    let staging = served.with_extension("bin.tmp");
    let result = std::fs::copy(base, &staging).and_then(|_| std::fs::rename(&staging, served));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&staging);
        eprintln!(
            "idiolect-app: cannot seed served model from {}: {e}",
            base.display()
        );
    }
}

/// Build the appropriate backend.
///
/// If stdin is a terminal the app was launched directly (standalone mode); we
/// own the sync server in-process via [`backend_local::LocalBackend`].
/// If stdin is a pipe the daemon spawned us (attached mode); we read snapshots
/// from that pipe via [`backend::PipeBackend`].
fn make_backend(rt: &tokio::runtime::Handle) -> Box<dyn Backend> {
    let standalone =
        std::io::stdin().is_terminal() || std::env::var_os("IDIOLECT_STANDALONE").is_some();
    if standalone {
        let store = standalone_store(
            data_dir(),
            env_path(dashboard_store_env::DB_PATH),
            env_path(dashboard_store_env::BASE_MODEL),
            env_path(dashboard_store_env::HISTORY_KEY),
        );
        let data = &store.data;
        for dir in [data.clone(), data.join("audio")] {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                eprintln!(
                    "idiolect-app: cannot create data dir {}: {e}",
                    dir.display()
                );
                std::process::exit(1);
            }
        }
        let pair_url = local_ip()
            .map(|ip| format!("http://{ip}:8765"))
            .unwrap_or_default();
        let cfg = standalone_sync_cfg(&store, pair_url);
        seed_served_model(&store.base_model, &cfg.model_path);
        let trainer_cfg = trainer_launcher::TrainerConfig {
            db_path: store.db_path.clone(),
            audio_root: data.join("audio"),
            base_model: store.base_model.clone(),
            output: data.join("personal.bin"),
            serve: Some(cfg.model_path.clone()),
            gpu: false,
        };
        match sync_host::SyncHost::start(cfg, rt) {
            Ok(host) => Box::new(backend_local::LocalBackend::new(host, Some(trainer_cfg))),
            Err(e) => {
                eprintln!("idiolect-app: cannot start sync server: {e}");
                std::process::exit(1);
            }
        }
    } else {
        Box::new(backend::PipeBackend::new())
    }
}

struct DashboardApp {
    backend: Box<dyn Backend>,
    model: DashboardModel,
    prefs_open: bool,
    /// Keeps the tokio runtime alive for the lifetime of the window.
    _rt: tokio::runtime::Runtime,
}

impl DashboardApp {
    fn new_with(mut backend: Box<dyn Backend>, rt: tokio::runtime::Runtime) -> Self {
        let snap = backend.poll_state().unwrap_or_default();
        Self {
            model: DashboardModel::from_snapshot(&snap),
            backend,
            prefs_open: false,
            _rt: rt,
        }
    }
}

impl eframe::App for DashboardApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Poll for a new snapshot (non-blocking).
        if let Some(snap) = self.backend.poll_state() {
            self.model = DashboardModel::from_snapshot(&snap);
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            // Top bar with prefs toggle.
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("idiolect")
                        .color(theme::PERIWINKLE)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("⚙").clicked() {
                        self.prefs_open = !self.prefs_open;
                    }
                });
            });
            ui.separator();

            let active_model = if self.prefs_open {
                DashboardModel {
                    screen: model::DashboardScreen::Prefs,
                    ..self.model.clone()
                }
            } else {
                self.model.clone()
            };

            let gestures = view::render(ui, &active_model);
            for gesture in gestures {
                if gesture == model::Gesture::OpenPrefs {
                    self.prefs_open = true;
                } else if gesture == model::Gesture::ClosePrefs {
                    self.prefs_open = false;
                } else if let Some(action) = DashboardModel::on_gesture(&gesture) {
                    self.backend.send(&action);
                }
            }
        });

        // Re-render at a modest rate for the pairing countdown.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::{local_ip, seed_served_model, standalone_store, standalone_sync_cfg};
    use std::path::PathBuf;

    #[test]
    fn the_sync_host_cfg_carries_the_stores_database_and_history_key() {
        // `make_backend` itself is a process boundary (fixed bind, real
        // sockets — see the note below); this seam pins that the resolved
        // store's fields actually REACH the host config. Dropping the history
        // key here would silently revert the dashboard to plaintext ingest on
        // an encrypted daemon store.
        let store = standalone_store(
            PathBuf::from("/data/root"),
            Some(PathBuf::from("/data/root/db/idiolect.sqlite")),
            Some(PathBuf::from("/data/root/models/whisper/ggml-base.en.bin")),
            Some(PathBuf::from("/data/root/db/history.key")),
        );
        let cfg = standalone_sync_cfg(&store, "http://10.0.0.5:8765".to_owned());
        assert_eq!(cfg.db_path, PathBuf::from("/data/root/db/idiolect.sqlite"));
        assert_eq!(
            cfg.history_key,
            Some(PathBuf::from("/data/root/db/history.key"))
        );
        assert_eq!(cfg.pair_url, "http://10.0.0.5:8765");
        // The served slot (trainer publish target == phone download source).
        assert_eq!(cfg.model_path, PathBuf::from("/data/root/model.bin"));
    }

    #[test]
    fn daemon_env_points_the_standalone_store_at_the_daemons_layout() {
        // The Linux tray spawns this app `--standalone` but hands over the
        // daemon's resolved store in the environment (the daemon's database is
        // db/idiolect.sqlite and its model lives under models/whisper/ — not
        // this app's own defaults). Honoring those keeps pairing and training
        // on the ONE store the daemon actually writes.
        let store = standalone_store(
            PathBuf::from("/data/root"),
            Some(PathBuf::from("/data/root/db/idiolect.sqlite")),
            Some(PathBuf::from("/data/root/models/whisper/ggml-base.en.bin")),
            Some(PathBuf::from("/data/root/db/history.key")),
        );
        assert_eq!(store.data, PathBuf::from("/data/root"));
        assert_eq!(
            store.db_path,
            PathBuf::from("/data/root/db/idiolect.sqlite")
        );
        assert_eq!(
            store.base_model,
            PathBuf::from("/data/root/models/whisper/ggml-base.en.bin")
        );
        // An encrypting daemon hands its key: sync ingest must cipher
        // ime_text_history rows, not write plaintext into its database.
        assert_eq!(
            store.history_key,
            Some(PathBuf::from("/data/root/db/history.key"))
        );
    }

    #[test]
    fn without_daemon_env_the_store_derives_from_the_data_dir() {
        // Direct standalone launches (macOS/Windows, or Linux without the
        // daemon) keep the historical single-directory layout.
        let store = standalone_store(
            PathBuf::from("/home/u/.local/share/idiolect"),
            None,
            None,
            None,
        );
        assert_eq!(
            store.db_path,
            PathBuf::from("/home/u/.local/share/idiolect/idiolect.db")
        );
        assert_eq!(
            store.base_model,
            PathBuf::from("/home/u/.local/share/idiolect/ggml-base.en.bin")
        );
        // No daemon ⇒ no cipher to honor: a direct launch's own store keeps
        // its historical plaintext posture.
        assert!(store.history_key.is_none());
    }

    // `make_backend` itself is the process-startup boundary (fixed 0.0.0.0:8765
    // bind, stdin probing), so the seeding rule is pinned here at the unit level
    // and the serving of whatever file occupies the slot is covered by the
    // SyncHost/LocalBackend suites.
    #[test]
    fn the_base_model_seeds_an_empty_served_slot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("ggml-base.en.bin");
        let served = dir.path().join("model.bin");
        std::fs::write(&base, b"base-model-bytes").expect("write base");

        seed_served_model(&base, &served);

        assert_eq!(
            std::fs::read(&served).expect("served must exist"),
            b"base-model-bytes",
            "a fresh install must serve the base model, not 404, until training \
             publishes a personal one"
        );
        assert!(
            !dir.path().join("model.bin.tmp").exists(),
            "the staging file must not be left behind"
        );
    }

    #[test]
    fn an_existing_served_model_is_never_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("ggml-base.en.bin");
        let served = dir.path().join("model.bin");
        std::fs::write(&base, b"base-model-bytes").expect("write base");
        std::fs::write(&served, b"trained-personal-model").expect("write served");

        seed_served_model(&base, &served);

        assert_eq!(
            std::fs::read(&served).expect("served"),
            b"trained-personal-model",
            "seeding must never clobber a trained model already in the slot"
        );
    }

    #[test]
    fn a_missing_base_model_is_a_quiet_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("ggml-base.en.bin");
        let served = dir.path().join("model.bin");

        seed_served_model(&base, &served);

        assert!(
            !served.exists(),
            "with nothing to seed from, the slot stays absent (a clean 404)"
        );
    }

    #[test]
    fn local_ip_returns_a_non_loopback_address_when_a_network_is_available() {
        // This test is best-effort: it passes if a LAN interface is reachable,
        // and is skipped-by-passing if not (e.g. in isolated CI without network).
        if let Some(ip) = local_ip() {
            assert!(!ip.is_loopback(), "pair URL should use a routable address");
            assert!(ip.is_ipv4() || ip.is_ipv6(), "must be a valid IP");
        }
        // None is also acceptable (offline environment); the caller falls back to "".
    }
}
