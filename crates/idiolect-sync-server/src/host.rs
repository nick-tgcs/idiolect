//! [`SyncHost`] — the embedded sync server that both the standalone
//! `idiolect-app` binary (macOS/Windows) and the Linux `idiolectd` daemon use
//! to manage phone pairing and ingest.
//!
//! Extracting this from the binary crate allows both deployments to share the
//! implementation without duplication.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::device_tokens::{DeviceTokenStore, PairedDevice};
use crate::pairing::{PairingOffer, PairingServerState};
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};

/// Configuration for a [`SyncHost`].
pub struct SyncHostConfig {
    /// The address the server binds to (e.g. `0.0.0.0:8765`).
    pub bind: SocketAddr,
    /// The URL the phone QR/pairing link will carry (phone-facing).
    pub pair_url: String,
    /// Whether to serve over TLS (self-signed; phone pins via SPKI).
    pub tls: bool,
    /// Path to the SQLite database.
    pub db_path: PathBuf,
    /// Root directory for audio files.
    pub audio_root: PathBuf,
    /// Path to the device token store JSON file.
    pub tokens_path: PathBuf,
}

/// An error starting or communicating with the embedded sync server.
#[derive(Debug, thiserror::Error)]
pub enum SyncHostError {
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
pub struct SyncHost {
    tokens: Arc<Mutex<DeviceTokenStore>>,
    pairing: Arc<PairingServerState>,
    store: Arc<Mutex<SqliteMetadataStore>>,
    pair_url: String,
    tls: bool,
}

impl SyncHost {
    /// Start the embedded sync server, binding to `cfg.bind`. The server runs
    /// for the lifetime of the tokio runtime (drop-to-shutdown is not yet
    /// wired — the tokio runtime is process-scoped).
    pub fn start(cfg: SyncHostConfig, rt: &tokio::runtime::Handle) -> Result<Self, SyncHostError> {
        let tokens = DeviceTokenStore::open(&cfg.tokens_path).map_err(SyncHostError::TokenStore)?;
        let tokens = Arc::new(Mutex::new(tokens));
        let pairing = Arc::new(PairingServerState::new(tokens.clone()));

        let mut store =
            SqliteMetadataStore::open_path(&cfg.db_path).map_err(SyncHostError::Database)?;
        store.migrate().map_err(SyncHostError::Database)?;
        let store = Arc::new(Mutex::new(store));
        let audio_store = FileAudioStore::new(cfg.audio_root.clone(), cfg.audio_root.clone());

        let model_cfg = Arc::new(crate::model_server::ModelServerConfig {
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
        let ingest_state = Arc::new(crate::ingest_server::IngestServerState::new(
            ingest_store,
            audio_store,
            tokens.clone(),
        ));

        let app = crate::build_app(model_cfg, pairing.clone(), Some(ingest_state));

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
    pub fn mint_pairing(&self, fingerprint: Option<&str>) -> Result<PairingOffer, SyncHostError> {
        let now = crate::pairing::system_now();
        self.pairing
            .mint_offer(&self.pair_url, fingerprint, now)
            .map_err(SyncHostError::Pairing)
    }

    /// List all devices that currently hold a valid token.
    pub fn paired_devices(&self) -> Vec<PairedDevice> {
        self.tokens.lock().expect("tokens").devices()
    }

    /// Revoke a paired device's token. Silent on I/O errors (logs to stderr).
    pub fn unpair(&self, device_id: &str) {
        if let Ok(mut tokens) = self.tokens.lock() {
            if let Err(e) = tokens.revoke(device_id) {
                eprintln!("idiolect-sync: unpair({device_id}): persist failed: {e}");
            }
        }
    }

    /// Test-only helper: register `device_id` directly in the token store, bypassing
    /// the full pairing handshake. Returns the plaintext bearer token.
    #[cfg(test)]
    pub(crate) fn issue_test_token(&self, device_id: &str) -> std::io::Result<String> {
        self.tokens
            .lock()
            .expect("tokens")
            .issue(device_id, "test-user")
    }

    /// Number of corrections waiting to be trained on.
    pub fn trainable_count(&self) -> u64 {
        self.store
            .lock()
            .expect("store")
            .trainable_count("default")
            .unwrap_or(0)
    }

    /// Timestamp of the last successful training run, if any.
    pub fn last_trained_at(&self) -> Option<String> {
        self.store
            .lock()
            .expect("store")
            .last_trained_at("default")
            .ok()
            .flatten()
    }

    /// Returns the phone-facing URL used in pairing QR codes.
    pub fn pair_url(&self) -> &str {
        &self.pair_url
    }

    /// Update the phone-facing URL (e.g. when the user sets it in Preferences).
    pub fn set_pair_url(&mut self, url: String) {
        self.pair_url = url;
    }

    /// Whether the server was started with TLS.
    pub fn tls(&self) -> bool {
        self.tls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_host(dir: &std::path::Path) -> SyncHost {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let cfg = SyncHostConfig {
            bind: "0.0.0.0:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.join("test.db"),
            audio_root: dir.join("audio"),
            tokens_path: dir.join("tokens.json"),
        };
        SyncHost::start(cfg, rt.handle()).expect("start")
        // rt is dropped here; the spawned task will be cancelled, but token
        // store and DB state remain and are what we're testing.
    }

    #[test]
    fn unpair_removes_device_and_revokes_its_token() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = test_host(dir.path());

        host.issue_test_token("pixel").expect("issue");
        assert_eq!(host.paired_devices().len(), 1, "device should be paired");

        host.unpair("pixel");

        assert!(
            host.paired_devices().is_empty(),
            "device should be removed after unpair"
        );
    }

    #[test]
    fn unpair_only_removes_targeted_device() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = test_host(dir.path());

        host.issue_test_token("pixel").expect("pixel");
        host.issue_test_token("tablet").expect("tablet");
        host.unpair("pixel");

        let ids: Vec<_> = host
            .paired_devices()
            .into_iter()
            .map(|d| d.device_id)
            .collect();
        assert_eq!(ids, ["tablet"], "only pixel should be removed");
    }
}
