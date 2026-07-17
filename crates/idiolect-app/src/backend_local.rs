//! [`LocalBackend`] — standalone mode (macOS/Windows): owns a [`SyncHost`]
//! in-process. No daemon subprocess required. The backend polls the SyncHost
//! for state changes and dispatches actions directly.

use idiolect_application::use_cases::sync_decision::should_auto_train;

use crate::backend::Backend;
use crate::model::{PairingSnapshot, Snapshot, TrainingProgress};
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
    /// Training prefs + latest progress line, owned HERE (the host knows nothing
    /// about training) — `refresh()` re-applies them over each rebuilt snapshot,
    /// exactly like `sync.enabled` lives in the host rather than the snapshot.
    auto_train_enabled: bool,
    auto_train_threshold: u32,
    training_progress: Option<TrainingProgress>,
}

impl LocalBackend {
    pub(crate) fn new(host: SyncHost, trainer_cfg: Option<TrainerConfig>) -> Self {
        let last = sync_host::snapshot(&host);
        let mut backend = Self {
            host,
            last,
            tick: 0,
            active_pairing: None,
            trainer_cfg,
            trainer: None,
            auto_train_watermark: 0,
            auto_train_enabled: false,
            auto_train_threshold: crate::model::default_auto_threshold(),
            training_progress: None,
        };
        // Compose the initial snapshot the same way every later one is built.
        backend.refresh();
        backend
    }

    fn refresh(&mut self) {
        self.last = sync_host::snapshot(&self.host);

        // Poll active trainer for progress / completion (mutating only the
        // backend-owned fields; they are applied to the snapshot below).
        if let Some(t) = &self.trainer {
            let poll = t.poll();
            if let Some(p) = poll.progress {
                self.training_progress = Some(p);
            }
            if let Some(result) = poll.done {
                if let Err(e) = result {
                    eprintln!("idiolect-app: training failed: {e}");
                }
                self.trainer = None;
                self.training_progress = None;
                // Advance the watermark to the current corrections count so that
                // a stale DB (trainerctl didn't decrement the counter) doesn't
                // cause an immediate re-trigger on the next refresh.
                self.auto_train_watermark = self.last.learning.new_corrections;
            }
        }

        // Training state is owned by this backend, not the host — re-apply it so
        // the rebuild above can't reset it (the same failure mode as sync:disable
        // flipping back, and what made auto-train unable to ever fire).
        self.last.training.auto_enabled = self.auto_train_enabled;
        self.last.training.auto_threshold = self.auto_train_threshold;
        self.last.training.running = self.trainer.is_some();
        self.last.training.progress = self.training_progress.clone();

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
                self.training_progress = None;
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
                self.host.set_enabled(true);
                self.last.sync.enabled = true;
            }
            "sync:disable" => {
                self.host.set_enabled(false);
                self.last.sync.enabled = false;
                self.active_pairing = None;
                self.last.pairing = PairingSnapshot::default();
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
                // Kill the code at the host too — clearing only the cached offer
                // would leave it redeemable at /v1/pair for the rest of its TTL.
                // (sync:disable needs no equivalent: set_enabled(false) invalidates.)
                self.host.cancel_pairing();
                self.active_pairing = None;
                self.last.pairing = PairingSnapshot::default();
            }
            "train:now" => self.start_training(),
            "train:auto:on" => {
                self.auto_train_enabled = true;
                self.last.training.auto_enabled = true;
            }
            "train:auto:off" => {
                self.auto_train_enabled = false;
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
                    .unwrap_or_else(|_| crate::model::default_auto_threshold());
                self.auto_train_threshold = n;
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
    fn sync_disable_survives_the_next_refresh() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = crate::sync_host::SyncHostConfig {
            bind: "0.0.0.0:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
        };
        let host = crate::sync_host::SyncHost::start(cfg, rt.handle()).expect("start");
        let mut backend = super::LocalBackend::new(host, None);

        backend.send("sync:disable");

        // The first poll tick forces a refresh, which rebuilds `last` from the host
        // snapshot — the disable must survive that rebuild (be host state), not flip
        // back to enabled within one refresh interval.
        let snap = backend.poll_state().expect("tick 1 refreshes");
        assert!(
            !snap.sync.enabled,
            "sync:disable must survive a snapshot refresh, not flip back to enabled"
        );
        assert!(!backend.host.enabled(), "disable must gate the host itself");

        backend.send("sync:enable");
        let snap = (0..60)
            .find_map(|_| backend.poll_state())
            .expect("a refresh tick within one interval");
        assert!(
            snap.sync.enabled,
            "sync:enable must survive a snapshot refresh symmetrically"
        );
    }

    /// One blocking `POST /v1/pair` claiming `code` against the backend's embedded
    /// host — the request a phone that scanned the QR would send.
    fn http_post_pair(addr: std::net::SocketAddr, code: &str) -> String {
        use std::io::{Read, Write};
        let body = format!(r#"{{"code":"{code}","device_id":"phone-under-test"}}"#);
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        write!(
            stream,
            "POST /v1/pair HTTP/1.1\r\nHost: idiolect-test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("write request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    }

    /// One blocking bearer-authenticated GET against the backend's embedded host —
    /// the request a paired phone sends to the model routes.
    fn http_get_authed(addr: std::net::SocketAddr, path: &str, token: &str) -> String {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: idiolect-test\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        )
        .expect("write request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    }

    #[test]
    fn a_freshly_paired_phone_downloads_the_model_the_dashboard_serves() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let model_bytes = b"personal-model-v1";
        std::fs::write(dir.path().join("model.bin"), model_bytes).expect("write model");
        let cfg = crate::sync_host::SyncHostConfig {
            bind: "127.0.0.1:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
        };
        let host = crate::sync_host::SyncHost::start(cfg, rt.handle()).expect("start");
        let mut backend = super::LocalBackend::new(host, None);

        // The whole onboarding a phone actually performs: the dashboard mints an
        // offer, the phone redeems the code for a bearer token, then pulls the model.
        backend.send("sync:pair");
        let code = backend
            .active_pairing
            .as_ref()
            .expect("sync:pair mints an offer")
            .code
            .clone();
        let pair = http_post_pair(backend.host.local_addr(), &code);
        assert!(
            pair.starts_with("HTTP/1.1 201"),
            "pairing must succeed, got: {}",
            pair.lines().next().unwrap_or_default()
        );
        let token = pair
            .split(r#""token":""#)
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("pair response carries the bearer token")
            .to_owned();

        let manifest = http_get_authed(backend.host.local_addr(), "/v1/model/manifest", &token);
        assert!(
            manifest.starts_with("HTTP/1.1 200"),
            "the paired phone's manifest call must succeed, got: {}",
            manifest.lines().next().unwrap_or_default()
        );
        let download = http_get_authed(backend.host.local_addr(), "/v1/model", &token);
        assert!(
            download.starts_with("HTTP/1.1 200"),
            "the paired phone's model download must succeed, got: {}",
            download.lines().next().unwrap_or_default()
        );
        assert!(
            download.ends_with(std::str::from_utf8(model_bytes).expect("ascii fixture")),
            "the phone must receive the file the dashboard serves"
        );
    }

    #[test]
    fn cancel_pair_invalidates_the_offer_at_the_host() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = crate::sync_host::SyncHostConfig {
            bind: "127.0.0.1:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
        };
        let host = crate::sync_host::SyncHost::start(cfg, rt.handle()).expect("start");
        let mut backend = super::LocalBackend::new(host, None);

        backend.send("sync:pair");
        let code = backend
            .active_pairing
            .as_ref()
            .expect("sync:pair mints an offer")
            .code
            .clone();
        backend.send("sync:cancel_pair");

        // Cancel removes the QR from the dashboard; the code it carried must be just
        // as dead at the host — the routes stay open, so clearing only the cached
        // offer would leave a code nobody can see redeemable for its full TTL.
        let response = http_post_pair(backend.host.local_addr(), &code);
        assert!(
            response.starts_with("HTTP/1.1 401"),
            "a cancelled pairing code must be refused at /v1/pair, got: {}",
            response.lines().next().unwrap_or_default()
        );
    }

    #[test]
    fn a_pairing_offer_hidden_by_sync_disable_is_dead_after_reenable() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = crate::sync_host::SyncHostConfig {
            bind: "127.0.0.1:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
        };
        let host = crate::sync_host::SyncHost::start(cfg, rt.handle()).expect("start");
        let mut backend = super::LocalBackend::new(host, None);

        backend.send("sync:pair");
        let code = backend
            .active_pairing
            .as_ref()
            .expect("sync:pair mints an offer")
            .code
            .clone();
        backend.send("sync:disable");
        backend.send("sync:enable");

        // Disable cleared the dashboard's offer; within the TTL the phone must not
        // be able to redeem the hidden code once the routes reopen.
        let response = http_post_pair(backend.host.local_addr(), &code);
        assert!(
            response.starts_with("HTTP/1.1 401"),
            "a code hidden by sync:disable must not redeem after sync:enable, got: {}",
            response.lines().next().unwrap_or_default()
        );
    }

    #[test]
    fn training_state_survives_the_next_refresh() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = crate::sync_host::SyncHostConfig {
            bind: "0.0.0.0:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
        };
        let host = crate::sync_host::SyncHost::start(cfg, rt.handle()).expect("start");
        let mut backend = super::LocalBackend::new(host, None);

        backend.send("train:auto:on");
        backend.send("train:auto_threshold:40");
        let (stub, _progress_tx, _done_tx) = crate::trainer_launcher::TrainerLauncher::test_stub();
        backend.trainer = Some(stub);

        // Same failure mode as sync:disable — these live only in the cached snapshot,
        // so the refresh's rebuild resets them (and the auto-train check then reads
        // the reset auto_enabled=false, so auto-train could never fire).
        let snap = backend.poll_state().expect("tick 1 refreshes");
        assert!(
            snap.training.auto_enabled,
            "the auto-train pref must survive a snapshot refresh"
        );
        assert_eq!(
            snap.training.auto_threshold, 40,
            "the auto-train threshold must survive a snapshot refresh"
        );
        assert!(
            snap.training.running,
            "a live trainer must keep training.running across refreshes"
        );
    }

    #[test]
    fn a_completed_training_run_clears_running_and_progress() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = crate::sync_host::SyncHostConfig {
            bind: "0.0.0.0:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
        };
        let host = crate::sync_host::SyncHost::start(cfg, rt.handle()).expect("start");
        let mut backend = super::LocalBackend::new(host, None);

        let (stub, progress_tx, done_tx) = crate::trainer_launcher::TrainerLauncher::test_stub();
        backend.trainer = Some(stub);
        progress_tx
            .send(crate::model::TrainingProgress {
                epoch: 2,
                epochs: 2,
                sample: 10,
                total: 10,
                loss_before: 0.0,
                loss_now: 0.2,
            })
            .expect("send progress");
        done_tx.send(Ok(())).expect("send done");

        // Progress and done can land on the SAME poll tick — the tick must still
        // end with the run over and nothing left on the progress bar.
        let snap = backend.poll_state().expect("tick 1 refreshes");
        assert!(!snap.training.running, "the done tick must end running");
        assert!(
            snap.training.progress.is_none(),
            "the done tick must clear progress"
        );
        assert!(
            backend.trainer.is_none(),
            "the trainer handle must be reaped"
        );
    }

    #[test]
    fn training_progress_persists_between_progress_lines() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = crate::sync_host::SyncHostConfig {
            bind: "0.0.0.0:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
        };
        let host = crate::sync_host::SyncHost::start(cfg, rt.handle()).expect("start");
        let mut backend = super::LocalBackend::new(host, None);

        let (stub, progress_tx, _done_tx) = crate::trainer_launcher::TrainerLauncher::test_stub();
        backend.trainer = Some(stub);
        progress_tx
            .send(crate::model::TrainingProgress {
                epoch: 1,
                epochs: 2,
                sample: 3,
                total: 10,
                loss_before: 0.0,
                loss_now: 1.0,
            })
            .expect("send progress");

        let snap = backend.poll_state().expect("tick 1 refreshes");
        assert!(
            snap.training.progress.is_some(),
            "a delivered progress line must reach the snapshot"
        );

        // No new line arrives before the next refresh — the bar must hold the last
        // value, not flicker back to empty because the rebuild reset it.
        let snap = (0..60)
            .find_map(|_| backend.poll_state())
            .expect("a refresh tick within one interval");
        assert!(
            snap.training.progress.is_some(),
            "progress must persist between trainer output lines"
        );
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
            model_path: dir.path().join("model.bin"),
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
