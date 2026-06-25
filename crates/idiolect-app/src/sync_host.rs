//! [`SyncHost`] — the embedded sync server for the standalone (macOS/Windows)
//! `idiolect-app` deployment. Wraps the existing `idiolect-sync-server` library,
//! providing a simple imperative API the `LocalBackend` can call without touching
//! axum/tokio internals.
//!
//! On Linux the daemon owns its own `SyncHost` (Phase 4); this module is used by
//! the standalone binary on macOS and Windows.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_sync_server::device_tokens::{DeviceTokenStore, PairedDevice};
use idiolect_sync_server::pairing::{PairingOffer, PairingServerState};

/// Configuration for a [`SyncHost`].
pub(crate) struct SyncHostConfig {
    /// The address the server binds to (e.g. `0.0.0.0:8765`).
    pub(crate) bind: SocketAddr,
    /// The URL the phone QR/pairing link will carry (phone-facing).
    pub(crate) pair_url: String,
    /// Whether to serve over TLS (self-signed; phone pins via SPKI).
    pub(crate) tls: bool,
    /// Path to the SQLite database.
    pub(crate) db_path: PathBuf,
    /// Root directory for audio files.
    pub(crate) audio_root: PathBuf,
    /// Path to the device token store JSON file.
    pub(crate) tokens_path: PathBuf,
}

/// An error starting or communicating with the embedded sync server.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SyncHostError {
    #[error("failed to open token store: {0}")]
    TokenStore(#[source] std::io::Error),
    #[error("failed to open database: {0}")]
    Database(#[source] idiolect_adapter_sqlite::SqliteStorageError),
    #[error("failed to bind listener: {0}")]
    Bind(#[source] std::io::Error),
    #[error("failed to mint pairing offer: {0}")]
    Pairing(String),
}

/// The live embedded sync server. Obtain via [`SyncHost::start`].
pub(crate) struct SyncHost {
    tokens: Arc<Mutex<DeviceTokenStore>>,
    pairing: Arc<PairingServerState>,
    store: Arc<Mutex<SqliteMetadataStore>>,
    pair_url: String,
    tls: bool,
}

impl SyncHost {
    /// Start the embedded sync server, binding to `cfg.bind`. The server runs
    /// for the lifetime of this struct (drop to shut down is not yet wired — the
    /// tokio runtime lives for the process lifetime).
    pub(crate) fn start(
        cfg: SyncHostConfig,
        rt: &tokio::runtime::Handle,
    ) -> Result<Self, SyncHostError> {
        let tokens = DeviceTokenStore::open(&cfg.tokens_path).map_err(SyncHostError::TokenStore)?;
        let tokens = Arc::new(Mutex::new(tokens));
        let pairing = Arc::new(PairingServerState::new(tokens.clone()));

        let mut store =
            SqliteMetadataStore::open_path(&cfg.db_path).map_err(SyncHostError::Database)?;
        store.migrate().map_err(SyncHostError::Database)?;
        let store = Arc::new(Mutex::new(store));
        let audio_store = FileAudioStore::new(cfg.audio_root.clone(), cfg.audio_root.clone());

        let model_cfg = Arc::new(idiolect_sync_server::model_server::ModelServerConfig {
            model_path: cfg
                .audio_root
                .parent()
                .unwrap_or(&cfg.audio_root)
                .to_path_buf(),
            model_id: "base.en".to_owned(),
            tokens: tokens.clone(),
        });

        let ingest_store =
            SqliteMetadataStore::open_path(&cfg.db_path).map_err(SyncHostError::Database)?;
        let ingest_state = Arc::new(idiolect_sync_server::ingest_server::IngestServerState::new(
            ingest_store,
            audio_store,
            tokens.clone(),
        ));

        let app = idiolect_sync_server::build_app(model_cfg, pairing.clone(), Some(ingest_state));

        let bind_addr: SocketAddr = cfg.bind;
        let listener = std::net::TcpListener::bind(bind_addr).map_err(SyncHostError::Bind)?;
        listener
            .set_nonblocking(true)
            .map_err(SyncHostError::Bind)?;

        rt.spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("convert listener");
            axum::serve(listener, app).await.ok();
        });

        Ok(Self {
            tokens,
            pairing,
            store,
            pair_url: cfg.pair_url,
            tls: cfg.tls,
        })
    }

    /// Mint a fresh pairing invitation (code + QR + expiry). The outstanding
    /// code supersedes any previous one.
    pub(crate) fn mint_pairing(
        &self,
        fingerprint: Option<&str>,
    ) -> Result<PairingOffer, SyncHostError> {
        let now = idiolect_sync_server::pairing::system_now();
        self.pairing
            .mint_offer(&self.pair_url, fingerprint, now)
            .map_err(SyncHostError::Pairing)
    }

    /// List all devices that currently hold a valid token.
    pub(crate) fn paired_devices(&self) -> Vec<PairedDevice> {
        self.tokens.lock().expect("tokens").devices()
    }

    /// Revoke a paired device's token.
    pub(crate) fn unpair(&self, device_id: &str) {
        // DeviceTokenStore has no `revoke` API yet; Phase 4 will add it.
        let _ = device_id;
    }

    /// Number of corrections waiting to be trained on (for the dashboard count).
    pub(crate) fn trainable_count(&self) -> u64 {
        self.store
            .lock()
            .expect("store")
            .trainable_count("default")
            .unwrap_or(0)
    }

    /// Timestamp of the last successful training run, if any.
    pub(crate) fn last_trained_at(&self) -> Option<String> {
        self.store
            .lock()
            .expect("store")
            .last_trained_at("default")
            .ok()
            .flatten()
    }

    /// Build a current [`crate::model::Snapshot`] from live server state.
    pub(crate) fn snapshot(&self) -> crate::model::Snapshot {
        let devices = self.paired_devices();
        let phones = devices
            .into_iter()
            .map(|d| crate::model::PhoneSnapshot {
                device_id: d.device_id.clone(),
                name: d.device_id.clone(),
                paired_at: d.issued_at.unwrap_or_default(),
            })
            .collect();

        crate::model::Snapshot {
            sync: crate::model::SyncSnapshot {
                enabled: true,
                reachable_url: self.pair_url.clone(),
                tls: self.tls,
            },
            phones,
            pairing: crate::model::PairingSnapshot::default(),
            learning: crate::model::LearningSnapshot {
                new_corrections: self.trainable_count(),
                last_trained_at: self.last_trained_at(),
            },
            training: crate::model::TrainingSnapshot::default(),
            model: crate::model::ModelSnapshot::default(),
        }
    }
}
