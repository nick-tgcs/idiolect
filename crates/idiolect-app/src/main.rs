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
    let standalone =
        std::io::stdin().is_terminal() || std::env::var_os("IDIOLECT_STANDALONE").is_some();
    if standalone {
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
        let pair_url = local_ip()
            .map(|ip| format!("http://{ip}:8765"))
            .unwrap_or_default();
        let cfg = sync_host::SyncHostConfig {
            bind: "0.0.0.0:8765".parse().expect("valid addr"),
            pair_url,
            tls: false,
            db_path: data.join("idiolect.db"),
            audio_root: data.join("audio"),
            tokens_path: data.join("device_tokens.json"),
        };
        let trainer_cfg = trainer_launcher::TrainerConfig {
            db_path: data.join("idiolect.db"),
            audio_root: data.join("audio"),
            base_model: data.join("ggml-base.en.bin"),
            output: data.join("personal.bin"),
            serve: Some(data.join("model.bin")),
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
    use super::local_ip;

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
