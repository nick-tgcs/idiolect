//! Pure dashboard model: deserialises the daemon/host state snapshot, drives the
//! view-model the egui layer renders, and maps user gestures to tray action-ids.
//! No I/O — fully deterministic and unit-tested here.

use serde::{Deserialize, Serialize};

// ── Snapshot types (daemon → app via stdin or LocalBackend) ───────────────────

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct Snapshot {
    #[serde(default)]
    pub(crate) sync: SyncSnapshot,
    #[serde(default)]
    pub(crate) phones: Vec<PhoneSnapshot>,
    #[serde(default)]
    pub(crate) pairing: PairingSnapshot,
    #[serde(default)]
    pub(crate) learning: LearningSnapshot,
    #[serde(default)]
    pub(crate) training: TrainingSnapshot,
    #[serde(default)]
    pub(crate) model: ModelSnapshot,
}

impl Snapshot {
    pub(crate) fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct SyncSnapshot {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) reachable_url: String,
    #[serde(default = "default_sync_tls")]
    pub(crate) tls: bool,
}

fn default_sync_tls() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct PhoneSnapshot {
    pub(crate) device_id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) paired_at: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct PairingSnapshot {
    #[serde(default)]
    pub(crate) active: bool,
    #[serde(default)]
    pub(crate) code: String,
    #[serde(default)]
    pub(crate) uri: String,
    /// Row-major bool grid (true = dark module).
    #[serde(default)]
    pub(crate) qr_matrix: Vec<bool>,
    #[serde(default)]
    pub(crate) qr_width: usize,
    #[serde(default)]
    pub(crate) expires_in_secs: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct LearningSnapshot {
    #[serde(default)]
    pub(crate) new_corrections: u64,
    #[serde(default)]
    pub(crate) last_trained_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct TrainingSnapshot {
    #[serde(default)]
    pub(crate) running: bool,
    #[serde(default)]
    pub(crate) auto_enabled: bool,
    #[serde(default = "default_auto_threshold")]
    pub(crate) auto_threshold: u32,
    #[serde(default)]
    pub(crate) progress: Option<TrainingProgress>,
}

pub(crate) fn default_auto_threshold() -> u32 {
    25
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct TrainingProgress {
    pub(crate) epoch: u32,
    pub(crate) epochs: u32,
    pub(crate) sample: u32,
    pub(crate) total: u32,
    pub(crate) loss_before: f32,
    pub(crate) loss_now: f32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct ModelSnapshot {
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) device: String,
}

// ── View-model produced from Snapshot ─────────────────────────────────────────

/// Which top-level screen the dashboard is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DashboardScreen {
    /// sync.enabled = false → prompt to enable
    SyncDisabled,
    /// sync.enabled, no phones → show "Pair a phone" button
    NoPhones,
    /// phones listed + correction / training status
    Phones,
    /// QR code being shown (pairing.active = true)
    PairingQr,
    /// Training is running
    Training,
    /// Preferences / URL config panel
    Prefs,
}

/// The view-model the egui layer renders; derived from a [`Snapshot`] by
/// [`DashboardModel::from_snapshot`].
#[derive(Debug, Clone)]
pub(crate) struct DashboardModel {
    pub(crate) screen: DashboardScreen,
    pub(crate) phones: Vec<PhoneSnapshot>,
    pub(crate) new_corrections: u64,
    pub(crate) last_trained_at: Option<String>,
    pub(crate) training_progress: Option<TrainingProgress>,
    pub(crate) auto_train: bool,
    pub(crate) auto_threshold: u32,
    pub(crate) pairing: PairingSnapshot,
    pub(crate) sync_url: String,
    pub(crate) sync_tls: bool,
    pub(crate) model_name: String,
    pub(crate) model_device: String,
}

/// A user gesture on the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Gesture {
    EnableSync,
    DisableSync,
    PairPhone,
    CancelPair,
    UnpairPhone(String),
    TrainNow,
    SetAutoTrain(bool),
    SetAutoThreshold(u32),
    SetReachableUrl(String),
    OpenPrefs,
    ClosePrefs,
}

impl DashboardModel {
    /// Build the view-model from a state snapshot.
    pub(crate) fn from_snapshot(snap: &Snapshot) -> Self {
        let screen = if snap.training.running {
            DashboardScreen::Training
        } else if snap.pairing.active {
            DashboardScreen::PairingQr
        } else if !snap.sync.enabled {
            DashboardScreen::SyncDisabled
        } else if snap.phones.is_empty() {
            DashboardScreen::NoPhones
        } else {
            DashboardScreen::Phones
        };

        Self {
            screen,
            phones: snap.phones.clone(),
            new_corrections: snap.learning.new_corrections,
            last_trained_at: snap.learning.last_trained_at.clone(),
            training_progress: snap.training.progress.clone(),
            auto_train: snap.training.auto_enabled,
            auto_threshold: snap.training.auto_threshold,
            pairing: snap.pairing.clone(),
            sync_url: snap.sync.reachable_url.clone(),
            sync_tls: snap.sync.tls,
            model_name: snap.model.name.clone(),
            model_device: snap.model.device.clone(),
        }
    }

    /// Map a user gesture to the action-id string the host handles. Returns
    /// `None` for gestures that are pure local UI state (no host effect needed).
    pub(crate) fn on_gesture(gesture: &Gesture) -> Option<String> {
        Some(match gesture {
            Gesture::EnableSync => "sync:enable".to_owned(),
            Gesture::DisableSync => "sync:disable".to_owned(),
            Gesture::PairPhone => "sync:pair".to_owned(),
            Gesture::CancelPair => "sync:cancel_pair".to_owned(),
            Gesture::UnpairPhone(id) => format!("sync:unpair:{id}"),
            Gesture::TrainNow => "train:now".to_owned(),
            Gesture::SetAutoTrain(true) => "train:auto:on".to_owned(),
            Gesture::SetAutoTrain(false) => "train:auto:off".to_owned(),
            Gesture::SetAutoThreshold(n) => format!("train:auto_threshold:{n}"),
            Gesture::SetReachableUrl(url) => format!("prefs:reachable_url:{url}"),
            Gesture::OpenPrefs | Gesture::ClosePrefs => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        Snapshot::default()
    }

    // ── screen derivation ──────────────────────────────────────────────────────

    #[test]
    fn sync_disabled_snapshot_shows_sync_disabled_screen() {
        let mut s = snap();
        s.sync.enabled = false;
        assert_eq!(
            DashboardModel::from_snapshot(&s).screen,
            DashboardScreen::SyncDisabled
        );
    }

    #[test]
    fn sync_enabled_no_phones_shows_no_phones_screen() {
        let mut s = snap();
        s.sync.enabled = true;
        assert_eq!(
            DashboardModel::from_snapshot(&s).screen,
            DashboardScreen::NoPhones
        );
    }

    #[test]
    fn sync_enabled_with_phones_shows_phones_screen() {
        let mut s = snap();
        s.sync.enabled = true;
        s.phones.push(PhoneSnapshot {
            device_id: "pixel".to_owned(),
            name: "Pixel".to_owned(),
            paired_at: String::new(),
        });
        assert_eq!(
            DashboardModel::from_snapshot(&s).screen,
            DashboardScreen::Phones
        );
    }

    #[test]
    fn active_pairing_takes_priority_over_phones_screen() {
        let mut s = snap();
        s.sync.enabled = true;
        s.phones.push(PhoneSnapshot {
            device_id: "pixel".to_owned(),
            name: "Pixel".to_owned(),
            paired_at: String::new(),
        });
        s.pairing.active = true;
        assert_eq!(
            DashboardModel::from_snapshot(&s).screen,
            DashboardScreen::PairingQr
        );
    }

    #[test]
    fn training_running_takes_priority_over_pairing_qr() {
        let mut s = snap();
        s.sync.enabled = true;
        s.pairing.active = true;
        s.training.running = true;
        assert_eq!(
            DashboardModel::from_snapshot(&s).screen,
            DashboardScreen::Training
        );
    }

    // ── field propagation ──────────────────────────────────────────────────────

    #[test]
    fn from_snapshot_propagates_correction_count() {
        let mut s = snap();
        s.learning.new_corrections = 42;
        assert_eq!(DashboardModel::from_snapshot(&s).new_corrections, 42);
    }

    #[test]
    fn from_snapshot_propagates_auto_train_settings() {
        let mut s = snap();
        s.training.auto_enabled = true;
        s.training.auto_threshold = 10;
        let m = DashboardModel::from_snapshot(&s);
        assert!(m.auto_train);
        assert_eq!(m.auto_threshold, 10);
    }

    #[test]
    fn from_snapshot_propagates_pairing_offer() {
        let mut s = snap();
        s.pairing.active = true;
        s.pairing.code = "ABCD1234".to_owned();
        s.pairing.expires_in_secs = 540;
        let m = DashboardModel::from_snapshot(&s);
        assert_eq!(m.pairing.code, "ABCD1234");
        assert_eq!(m.pairing.expires_in_secs, 540);
    }

    // ── round-trip serialisation ───────────────────────────────────────────────

    #[test]
    fn snapshot_round_trips_through_json() {
        let mut s = snap();
        s.sync.enabled = true;
        s.learning.new_corrections = 7;
        s.phones.push(PhoneSnapshot {
            device_id: "phone-1".to_owned(),
            name: "My Phone".to_owned(),
            paired_at: "2026-06-25T10:00:00Z".to_owned(),
        });
        let json = serde_json::to_string(&s).expect("serializes");
        let back = Snapshot::from_json(&json).expect("round-trip");
        assert!(back.sync.enabled);
        assert_eq!(back.learning.new_corrections, 7);
        assert_eq!(back.phones[0].device_id, "phone-1");
    }

    #[test]
    fn snapshot_from_empty_json_object_uses_all_defaults() {
        let s = Snapshot::from_json("{}").expect("parse");
        assert!(!s.sync.enabled);
        assert!(s.phones.is_empty());
        assert!(!s.pairing.active);
    }

    // ── gesture → action-id ───────────────────────────────────────────────────

    #[test]
    fn enable_sync_maps_to_sync_enable() {
        assert_eq!(
            DashboardModel::on_gesture(&Gesture::EnableSync),
            Some("sync:enable".to_owned()),
        );
    }

    #[test]
    fn disable_sync_maps_to_sync_disable() {
        assert_eq!(
            DashboardModel::on_gesture(&Gesture::DisableSync),
            Some("sync:disable".to_owned()),
        );
    }

    #[test]
    fn pair_phone_maps_to_sync_pair() {
        assert_eq!(
            DashboardModel::on_gesture(&Gesture::PairPhone),
            Some("sync:pair".to_owned()),
        );
    }

    #[test]
    fn cancel_pair_maps_to_sync_cancel_pair() {
        assert_eq!(
            DashboardModel::on_gesture(&Gesture::CancelPair),
            Some("sync:cancel_pair".to_owned()),
        );
    }

    #[test]
    fn unpair_embeds_device_id_in_action() {
        assert_eq!(
            DashboardModel::on_gesture(&Gesture::UnpairPhone("pixel-8".to_owned())),
            Some("sync:unpair:pixel-8".to_owned()),
        );
    }

    #[test]
    fn train_now_maps_to_train_now() {
        assert_eq!(
            DashboardModel::on_gesture(&Gesture::TrainNow),
            Some("train:now".to_owned()),
        );
    }

    #[test]
    fn auto_train_on_maps_to_train_auto_on() {
        assert_eq!(
            DashboardModel::on_gesture(&Gesture::SetAutoTrain(true)),
            Some("train:auto:on".to_owned()),
        );
    }

    #[test]
    fn auto_train_off_maps_to_train_auto_off() {
        assert_eq!(
            DashboardModel::on_gesture(&Gesture::SetAutoTrain(false)),
            Some("train:auto:off".to_owned()),
        );
    }

    #[test]
    fn auto_threshold_embeds_the_count_in_action() {
        assert_eq!(
            DashboardModel::on_gesture(&Gesture::SetAutoThreshold(10)),
            Some("train:auto_threshold:10".to_owned()),
        );
    }

    #[test]
    fn open_and_close_prefs_produce_no_action() {
        assert_eq!(DashboardModel::on_gesture(&Gesture::OpenPrefs), None);
        assert_eq!(DashboardModel::on_gesture(&Gesture::ClosePrefs), None);
    }
}
