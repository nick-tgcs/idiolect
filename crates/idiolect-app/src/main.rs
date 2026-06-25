//! Idiolect dashboard: phone↔PC sync status, pairing QR, and training controls.
//!
//! Subprocess contract (attached / Linux mode, daemon's `SyncPanelLauncher` drives this):
//!   stdin  : newline-delimited JSON state snapshots (see `model::Snapshot`)
//!   stdout : one action-id per line (`sync:pair`, `train:now`, …)
//!
//! Standalone mode (macOS / Windows): no daemon; the app owns its own `SyncHost`.

use std::io::IsTerminal;
use std::path::PathBuf;

use eframe::egui;

mod backend;
mod backend_local;
mod model;
mod sync_host;
mod theme;
mod view;

use backend::Backend;
use model::DashboardModel;

fn main() -> eframe::Result<()> {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
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

/// Returns the data directory for standalone mode.
fn data_dir() -> PathBuf {
    std::env::var_os("IDIOLECT_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/idiolect"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/idiolect"))
}

/// Build the appropriate backend.
///
/// If stdin is a terminal the app was launched directly (standalone mode); we
/// own the sync server in-process via [`backend_local::LocalBackend`].
/// If stdin is a pipe the daemon spawned us (attached mode); we read snapshots
/// from that pipe via [`backend::PipeBackend`].
fn make_backend(rt: &tokio::runtime::Handle) -> Box<dyn Backend> {
    if std::io::stdin().is_terminal() {
        let data = data_dir();
        for dir in [&data, &data.join("audio")] {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!(
                    "idiolect-app: cannot create data dir {}: {e}",
                    dir.display()
                );
                std::process::exit(1);
            }
        }
        let cfg = sync_host::SyncHostConfig {
            bind: "0.0.0.0:8765".parse().expect("valid addr"),
            pair_url: String::new(),
            tls: false,
            db_path: data.join("idiolect.db"),
            audio_root: data.join("audio"),
            tokens_path: data.join("device_tokens.json"),
        };
        match sync_host::SyncHost::start(cfg, rt) {
            Ok(host) => Box::new(backend_local::LocalBackend::new(host)),
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll for a new snapshot (non-blocking).
        if let Some(snap) = self.backend.poll_state() {
            self.model = DashboardModel::from_snapshot(&snap);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
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
