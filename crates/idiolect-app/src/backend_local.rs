//! [`LocalBackend`] — standalone mode (macOS/Windows): owns a [`SyncHost`]
//! in-process. No daemon subprocess required. The backend polls the SyncHost
//! for state changes and dispatches actions directly.

use idiolect_application::use_cases::sync_decision::should_auto_train;

use crate::backend::Backend;
use crate::model::{PairingSnapshot, Snapshot};
use crate::sync_host::{self, SyncHost};
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
    /// The `new_corrections` count at the last auto-train trigger (or training
    /// completion). Auto-train only fires again when the count exceeds this
    /// watermark — prevents an endless retrain loop when `trainerctl` does not
    /// decrement the corrections counter in the database.
    auto_train_watermark: u64,
}

impl LocalBackend {
    pub(crate) fn new(host: SyncHost, trainer_cfg: Option<TrainerConfig>) -> Self {
        let last = sync_host::snapshot(&host);
        Self {
            host,
            last,
            tick: 0,
            active_pairing: None,
            trainer_cfg,
            trainer: None,
            auto_train_watermark: 0,
        }
    }

    fn refresh(&mut self) {
        self.last = sync_host::snapshot(&self.host);

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
                // Advance the watermark to the current corrections count so that
                // a stale DB (trainerctl didn't decrement the counter) doesn't
                // cause an immediate re-trigger on the next refresh.
                self.auto_train_watermark = self.last.learning.new_corrections;
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

        // Check auto-train: trigger only when the corrections count has grown
        // beyond the last-triggered watermark, preventing an endless retrain
        // loop if trainerctl leaves the count unchanged in the database.
        let count = self.last.learning.new_corrections;
        if self.trainer.is_none()
            && count > self.auto_train_watermark
            && should_auto_train(
                self.last.training.auto_enabled,
                self.last.training.auto_threshold,
                count,
            )
        {
            self.auto_train_watermark = count;
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
                self.host.set_pair_url(url.to_owned());
                self.last.sync.reachable_url = url.to_owned();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::backend::Backend;
    use idiolect_application::use_cases::sync_decision::should_auto_train;

    /// Mirrors the watermark guard in `LocalBackend::refresh()` so it can be
    /// tested as a pure function without starting a real SyncHost.
    fn should_trigger_auto_train(
        trainer_idle: bool,
        count: u64,
        watermark: u64,
        auto_enabled: bool,
        threshold: u32,
    ) -> bool {
        trainer_idle && count > watermark && should_auto_train(auto_enabled, threshold, count)
    }

    #[test]
    fn auto_train_fires_when_count_exceeds_zero_watermark() {
        assert!(should_trigger_auto_train(true, 30, 0, true, 25));
    }

    #[test]
    fn auto_train_does_not_retrigger_after_training_when_count_unchanged() {
        // Simulate: training completed, watermark advanced to 30, DB still shows 30.
        let watermark = 30_u64;
        let count = 30_u64;
        assert!(
            !should_trigger_auto_train(true, count, watermark, true, 25),
            "must not retrigger when count == watermark (stale DB)"
        );
    }

    #[test]
    fn auto_train_retrigggers_when_new_corrections_arrive_after_training() {
        // After training watermark = 30; new corrections push count to 35.
        assert!(should_trigger_auto_train(true, 35, 30, true, 25));
    }

    #[test]
    fn auto_train_does_not_fire_while_trainer_is_running() {
        assert!(!should_trigger_auto_train(false, 100, 0, true, 25));
    }

    #[test]
    fn auto_train_respects_threshold_below_watermark() {
        // count=15 doesn't meet threshold=25, even though count > watermark=0.
        assert!(!should_trigger_auto_train(true, 15, 0, true, 25));
    }

    #[test]
    fn prefs_reachable_url_propagates_to_host_pair_url() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = crate::sync_host::SyncHostConfig {
            bind: "0.0.0.0:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            tokens_path: dir.path().join("tokens.json"),
        };
        let host = crate::sync_host::SyncHost::start(cfg, rt.handle()).expect("start");
        let mut backend = super::LocalBackend::new(host, None);

        backend.send("prefs:reachable_url:http://192.168.1.42:8765");

        assert_eq!(
            backend.host.pair_url(),
            "http://192.168.1.42:8765",
            "pair_url must be updated in the host, not just the snapshot"
        );
        assert_eq!(
            backend.last.sync.reachable_url, "http://192.168.1.42:8765",
            "snapshot must also reflect the new url"
        );
    }
}
