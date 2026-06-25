//! [`LocalBackend`] — standalone mode (macOS/Windows): owns a [`SyncHost`]
//! in-process. No daemon subprocess required. The backend polls the SyncHost
//! for state changes and dispatches actions directly.

use idiolect_application::use_cases::sync_decision::should_auto_train;

use crate::backend::Backend;
use crate::model::{PairingSnapshot, Snapshot};
use crate::sync_host::SyncHost;
use crate::trainer_launcher::{TrainerConfig, TrainerLauncher};

/// The standalone backend: owns the embedded sync server and drives the
/// dashboard directly. Used on macOS and Windows (and optionally Linux when
/// running without the IBus daemon).
pub(crate) struct LocalBackend {
    host: SyncHost,
    /// Cached snapshot; rebuilt on every action that may have changed state.
    last: Snapshot,
    /// How many poll ticks between forced refreshes (each tick is ~16ms / 60fps).
    tick: u32,
    active_pairing: Option<idiolect_sync_server::pairing::PairingOffer>,
    /// Config for spawning `idiolect-trainerctl`. `None` disables training.
    trainer_cfg: Option<TrainerConfig>,
    /// A live trainer subprocess, if training is in progress.
    trainer: Option<TrainerLauncher>,
}

impl LocalBackend {
    pub(crate) fn new(host: SyncHost, trainer_cfg: Option<TrainerConfig>) -> Self {
        let last = host.snapshot();
        Self {
            host,
            last,
            tick: 0,
            active_pairing: None,
            trainer_cfg,
            trainer: None,
        }
    }

    fn refresh(&mut self) {
        self.last = self.host.snapshot();

        // Poll active trainer for progress / completion.
        if let Some(t) = &self.trainer {
            let poll = t.poll();
            if let Some(p) = poll.progress {
                self.last.training.progress = Some(p);
            }
            if let Some(result) = poll.done {
                if let Err(e) = result {
                    eprintln!("idiolect-app: training failed: {e}");
                }
                self.trainer = None;
                self.last.training.running = false;
                self.last.training.progress = None;
            }
        }

        // Propagate pairing offer state (countdown / expiry).
        if let Some(offer) = &self.active_pairing {
            let now = idiolect_sync_server::pairing::system_now();
            if now >= offer.expires_at_secs {
                self.active_pairing = None;
                self.last.pairing = PairingSnapshot::default();
            } else {
                self.last.pairing = PairingSnapshot {
                    active: true,
                    code: offer.display_code.clone(),
                    uri: offer.uri.clone(),
                    qr_matrix: offer.qr_matrix.clone(),
                    qr_width: offer.qr_width,
                    expires_in_secs: offer.expires_at_secs.saturating_sub(now),
                };
            }
        }

        // Check auto-train: trigger if enabled, threshold met, and no run active.
        if self.trainer.is_none()
            && should_auto_train(
                self.last.training.auto_enabled,
                self.last.training.auto_threshold,
                self.last.learning.new_corrections,
            )
        {
            self.start_training();
        }
    }

    /// Spawn `idiolect-trainerctl` if a `TrainerConfig` is available.
    fn start_training(&mut self) {
        let Some(cfg) = &self.trainer_cfg else {
            return;
        };
        match TrainerLauncher::start(cfg) {
            Ok(t) => {
                self.trainer = Some(t);
                self.last.training.running = true;
                self.last.training.progress = None;
            }
            Err(e) => eprintln!("idiolect-app: cannot start trainer: {e}"),
        }
    }
}

impl Backend for LocalBackend {
    fn poll_state(&mut self) -> Option<Snapshot> {
        self.tick = self.tick.wrapping_add(1);
        // Refresh every ~30 ticks (≈500 ms at 60fps) or on first call.
        if self.tick.is_multiple_of(30) || self.tick == 1 {
            self.refresh();
            Some(self.last.clone())
        } else {
            None
        }
    }

    fn send(&mut self, action: &str) {
        match action {
            "sync:enable" => {
                self.last.sync.enabled = true;
            }
            "sync:disable" => {
                self.last.sync.enabled = false;
                self.active_pairing = None;
            }
            "sync:pair" => match self.host.mint_pairing(None) {
                Ok(offer) => {
                    let now = idiolect_sync_server::pairing::system_now();
                    self.last.pairing = PairingSnapshot {
                        active: true,
                        code: offer.display_code.clone(),
                        uri: offer.uri.clone(),
                        qr_matrix: offer.qr_matrix.clone(),
                        qr_width: offer.qr_width,
                        expires_in_secs: offer.expires_at_secs.saturating_sub(now),
                    };
                    self.active_pairing = Some(offer);
                }
                Err(err) => eprintln!("idiolect-app: mint_pairing: {err}"),
            },
            "sync:cancel_pair" => {
                self.active_pairing = None;
                self.last.pairing = PairingSnapshot::default();
            }
            "train:now" => self.start_training(),
            "train:auto:on" => {
                self.last.training.auto_enabled = true;
            }
            "train:auto:off" => {
                self.last.training.auto_enabled = false;
            }
            _ if action.starts_with("sync:unpair:") => {
                let device_id = &action["sync:unpair:".len()..];
                self.host.unpair(device_id);
                self.refresh();
            }
            _ if action.starts_with("train:auto_threshold:") => {
                let n: u32 = action["train:auto_threshold:".len()..]
                    .parse()
                    .unwrap_or(25);
                self.last.training.auto_threshold = n;
            }
            _ if action.starts_with("prefs:reachable_url:") => {
                let url = &action["prefs:reachable_url:".len()..];
                self.last.sync.reachable_url = url.to_owned();
            }
            _ => {}
        }
    }
}
