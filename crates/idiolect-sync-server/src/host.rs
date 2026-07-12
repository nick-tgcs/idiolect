//! [`SyncHost`] — the embedded sync server that both the standalone
//! `idiolect-app` binary (macOS/Windows) and the Linux `idiolectd` daemon use
//! to manage phone pairing and ingest.
//!
//! Extracting this from the binary crate allows both deployments to share the
//! implementation without duplication.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::response::IntoResponse;

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
    #[error(
        "embedded sync host does not terminate TLS (tls:true); serve cleartext (tls:false) or run the standalone idiolect-sync-server"
    )]
    TlsUnsupported,
    #[error("sync is disabled")]
    Disabled,
}

/// The live embedded sync server. Obtain via [`SyncHost::start`].
pub struct SyncHost {
    tokens: Arc<Mutex<DeviceTokenStore>>,
    pairing: Arc<PairingServerState>,
    store: Arc<Mutex<SqliteMetadataStore>>,
    pair_url: String,
    tls: bool,
    /// Shared with the serving task's gate middleware: `false` ⇒ every phone-facing
    /// route answers 503 and no pairing offer can be minted.
    enabled: Arc<AtomicBool>,
    local_addr: SocketAddr,
}

impl SyncHost {
    /// Start the embedded sync server, binding to `cfg.bind`. The server runs
    /// for the lifetime of the tokio runtime (drop-to-shutdown is not yet
    /// wired — the tokio runtime is process-scoped).
    pub fn start(cfg: SyncHostConfig, rt: &tokio::runtime::Handle) -> Result<Self, SyncHostError> {
        // This host serves the router with plain `axum::serve` below; it has no TLS acceptor.
        // Silently downgrading a tls:true request to cleartext would advertise TLS on the
        // dashboard while any https/pinning pairing URL fails its handshake — so refuse loudly
        // instead of lying. TLS termination lives in the standalone `idiolect-sync-server`.
        if cfg.tls {
            return Err(SyncHostError::TlsUnsupported);
        }
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

        // The dashboard's "Disable Sync" must actually stop the server from accepting
        // phone traffic (pairing, ingest, model pulls) — not just relabel the UI — so
        // every route is gated on this flag. The listener stays bound; re-enabling is
        // instant and needs no rebind.
        let enabled = Arc::new(AtomicBool::new(true));
        let gate = enabled.clone();
        let app = app.layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let gate = gate.clone();
                async move {
                    if gate.load(Ordering::SeqCst) {
                        next.run(req).await
                    } else {
                        (
                            axum::http::StatusCode::SERVICE_UNAVAILABLE,
                            "sync is disabled\n",
                        )
                            .into_response()
                    }
                }
            },
        ));

        let bind_addr: SocketAddr = cfg.bind;
        let listener = std::net::TcpListener::bind(bind_addr).map_err(SyncHostError::Bind)?;
        let local_addr = listener.local_addr().map_err(SyncHostError::Bind)?;
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
            enabled,
            local_addr,
        })
    }

    /// Mint a fresh pairing invitation (code + QR + expiry). The outstanding
    /// code supersedes any previous one. Refused while the host is disabled —
    /// the gated routes could never serve the claim anyway.
    pub fn mint_pairing(&self, fingerprint: Option<&str>) -> Result<PairingOffer, SyncHostError> {
        if !self.enabled() {
            return Err(SyncHostError::Disabled);
        }
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

    /// Whether the host is currently serving phone traffic.
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Enable/disable serving. While disabled every phone-facing route answers 503
    /// and [`SyncHost::mint_pairing`] refuses; the listener stays bound so
    /// re-enabling is instant.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
    }

    /// The address the server is actually bound to (resolves a `:0` bind).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
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
    fn start_refuses_tls_because_the_embedded_host_serves_cleartext_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let cfg = SyncHostConfig {
            bind: "0.0.0.0:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: true,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            tokens_path: dir.path().join("tokens.json"),
        };
        // Rather than bind a cleartext socket while reporting TLS enabled, start must fail.
        // (SyncHost is not Debug, so match on the Result instead of `expect_err`.)
        let result = SyncHost::start(cfg, rt.handle());
        assert!(
            matches!(result, Err(SyncHostError::TlsUnsupported)),
            "tls:true must be rejected with TlsUnsupported",
        );
    }

    /// One blocking HTTP/1.1 request, dependency-free (the status line is all we assert on).
    fn http_get(addr: SocketAddr, path: &str) -> String {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: idiolect-test\r\nConnection: close\r\n\r\n"
        )
        .expect("write request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    }

    #[test]
    fn disabling_the_host_gates_phone_routes_until_reenabled() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = SyncHostConfig {
            bind: "127.0.0.1:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            tokens_path: dir.path().join("tokens.json"),
        };
        let host = SyncHost::start(cfg, rt.handle()).expect("start");
        let addr = host.local_addr();

        assert!(
            !http_get(addr, "/v1/model/manifest").starts_with("HTTP/1.1 503"),
            "an enabled host must serve phone routes"
        );

        host.set_enabled(false);
        assert!(
            http_get(addr, "/v1/model/manifest").starts_with("HTTP/1.1 503"),
            "a disabled host must refuse phone traffic, not serve while the UI says off"
        );

        host.set_enabled(true);
        assert!(
            !http_get(addr, "/v1/model/manifest").starts_with("HTTP/1.1 503"),
            "re-enabling must restore serving without a rebind"
        );
    }

    #[test]
    fn mint_pairing_refuses_while_disabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = test_host(dir.path());

        host.set_enabled(false);

        assert!(
            matches!(host.mint_pairing(None), Err(SyncHostError::Disabled)),
            "a disabled host must not mint pairing offers its gated routes cannot serve"
        );
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
