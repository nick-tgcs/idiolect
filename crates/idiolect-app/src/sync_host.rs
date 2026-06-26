//! Re-exports [`SyncHost`] from the shared library and adds app-specific helpers.
pub(crate) use idiolect_sync_server::host::{SyncHost, SyncHostConfig};

use crate::model::{
    LearningSnapshot, ModelSnapshot, PairingSnapshot, PhoneSnapshot, Snapshot, SyncSnapshot,
    TrainingSnapshot,
};

/// Build a current [`Snapshot`] from live server state.
pub(crate) fn snapshot(host: &SyncHost) -> Snapshot {
    let devices = host.paired_devices();
    let phones = devices
        .into_iter()
        .map(|d| PhoneSnapshot {
            device_id: d.device_id.clone(),
            name: d.device_id.clone(),
            paired_at: d.issued_at.unwrap_or_default(),
        })
        .collect();
    Snapshot {
        sync: SyncSnapshot {
            enabled: true,
            reachable_url: host.pair_url().to_owned(),
            tls: host.tls(),
        },
        phones,
        pairing: PairingSnapshot::default(),
        learning: LearningSnapshot {
            new_corrections: host.trainable_count(),
            last_trained_at: host.last_trained_at(),
        },
        training: TrainingSnapshot::default(),
        model: ModelSnapshot::default(),
    }
}
