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
use idiolect_adapter_crypto::{ChaCha20Poly1305Cipher, FileKey};
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use std::path::Path;

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
    /// Path to the model file served to paired phones (`/v1/model`).
    pub model_path: PathBuf,
    /// Path to the device token store JSON file.
    pub tokens_path: PathBuf,
    /// The at-rest history key file of the daemon that owns `db_path`, when
    /// that daemon encrypts history (`None` ⇒ plaintext, the standalone
    /// default). Sync ingest writes `ime_text_history` rows via
    /// `commit_session`; opening the daemon's database without its cipher
    /// would land phone corrections as plaintext in an encrypted store.
    pub history_key: Option<PathBuf>,
}

/// An error starting or communicating with the embedded sync server.
#[derive(Debug, thiserror::Error)]
pub enum SyncHostError {
    #[error("failed to open token store: {0}")]
    TokenStore(#[source] std::io::Error),
    #[error("failed to open database: {0}")]
    Database(#[source] idiolect_adapter_sqlite::SqliteStorageError),
    #[error("failed to load history key: {0}")]
    HistoryKey(#[source] idiolect_adapter_crypto::CryptoError),
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

/// The ONE way a sync server opens its metadata store: with the owning
/// daemon's history cipher when a key file is configured
/// ([`SyncHostConfig::history_key`]), plaintext otherwise. Every open in
/// [`SyncHost::start`] — and the standalone binary's ingest open — must go
/// through here: a bare `open_path` on an encrypted daemon database would
/// write plaintext `ime_text_history` rows on ingest.
pub fn open_store(
    db_path: &Path,
    history_key: Option<&Path>,
) -> Result<SqliteMetadataStore, SyncHostError> {
    let store = SqliteMetadataStore::open_path(db_path).map_err(SyncHostError::Database)?;
    match history_key {
        Some(key_path) => {
            // Load-ONLY: this host borrows the owning daemon's key (created at
            // daemon startup, before any dashboard can spawn). A missing file
            // fails the start loudly — minting a key here would fork the
            // keyspace under the daemon.
            let key = FileKey::new(key_path)
                .load_key()
                .map_err(SyncHostError::HistoryKey)?;
            Ok(store.with_history_cipher(Box::new(ChaCha20Poly1305Cipher::new(key))))
        }
        None => Ok(store),
    }
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

        let mut store = open_store(&cfg.db_path, cfg.history_key.as_deref())?;
        store.migrate().map_err(SyncHostError::Database)?;
        let store = Arc::new(Mutex::new(store));
        let audio_store = FileAudioStore::new(cfg.audio_root.clone(), cfg.audio_root.clone());

        let model_cfg = Arc::new(crate::model_server::ModelServerConfig {
            model_path: cfg.model_path.clone(),
            model_id: "base.en".to_owned(),
            tokens: tokens.clone(),
        });

        let ingest_store = open_store(&cfg.db_path, cfg.history_key.as_deref())?;
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
    ///
    /// Mint and [`set_enabled`](Self::set_enabled) must be called from one thread
    /// (the dashboard's UI thread today): the enabled check here and the
    /// disable-side invalidation are not one atomic step, so concurrent callers
    /// could re-arm a code on a disabled host.
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
    /// re-enabling is instant. Disabling also invalidates any outstanding pairing
    /// offer: the dashboard stops showing it, so it must not stay silently
    /// redeemable when the routes reopen within its TTL.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
        if !on {
            self.pairing.cancel_pending();
        }
    }

    /// Invalidate the outstanding pairing offer (the dashboard's "Cancel"). The
    /// routes stay open — only the code dies; without this a cancelled code would
    /// stay redeemable until its TTL even though the UI no longer shows it.
    pub fn cancel_pairing(&self) {
        self.pairing.cancel_pending();
    }

    /// The address the server is actually bound to (resolves a `:0` bind).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idiolect_ports::storage::MetadataStorePort;

    #[test]
    fn open_store_with_a_history_key_encrypts_history_at_rest_like_the_daemon() {
        // The store this host is handed may be a daemon's encrypted database
        // (the tray dashboard case): rows this host writes through
        // `commit_session` must cipher `ime_text_history` exactly as the
        // daemon does, or phone corrections land readable on disk.
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("idiolect.sqlite");
        let key = dir.path().join("history.key");
        std::fs::write(&key, [7_u8; 32]).expect("the daemon pre-created its key");

        let mut store = open_store(&db, Some(&key)).expect("open ciphered");
        store.migrate().expect("migrate");
        let session = store
            .create_session(Some("phone corrected 1234"))
            .expect("session");
        store
            .commit_session(session, "phone corrected 1234", "ingest:test:1")
            .expect("commit");

        // At rest: a cipher-less open shows the raw column — it must not be
        // the plaintext (same idiom as the adapter's encryption contract).
        let plain = open_store(&db, None).expect("open plain");
        let stored = plain.recent_history(10).expect("history")[0].text.clone();
        assert_ne!(stored, "phone corrected 1234");
        assert!(!stored.contains("corrected"));

        // Same key file: the daemon's own view round-trips the plaintext.
        let same = open_store(&db, Some(&key)).expect("reopen ciphered");
        assert_eq!(
            same.recent_history(10).expect("history")[0].text,
            "phone corrected 1234"
        );
    }

    #[test]
    fn start_fails_loudly_when_the_handed_over_key_is_missing() {
        // The daemon OWNS the key: it creates the file at startup, before its
        // tray can spawn a dashboard. This host only BORROWS it — if the file
        // is missing, minting a fresh one here would silently fork the
        // keyspace (rows this host ingests become unreadable to the daemon
        // that handed the path over, and a daemon restart then orphans all
        // earlier history). Refusing to start is the honest answer.
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let key = dir.path().join("history.key");
        let cfg = SyncHostConfig {
            bind: "127.0.0.1:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
            history_key: Some(key.clone()),
        };
        let result = SyncHost::start(cfg, rt.handle());
        assert!(
            matches!(result, Err(SyncHostError::HistoryKey(_))),
            "a missing borrowed key must refuse the start, not mint a new key",
        );
        assert!(
            !key.exists(),
            "the borrower must never create the owner's key"
        );
    }

    #[test]
    fn start_without_a_history_key_creates_none() {
        // A direct standalone launch (its own store, no daemon) stays
        // plaintext — no stray key file that a later daemon might pick up.
        let dir = tempfile::tempdir().expect("tempdir");
        let _host = test_host(dir.path());
        assert!(!dir.path().join("history.key").exists());
    }

    /// One encoded sync batch carrying a single corrected learning — the body a
    /// paired phone POSTs to `/v1/sync`.
    fn sync_batch_bytes(digest: &str, raw: &str, corrected: &str) -> Vec<u8> {
        use idiolect_sync::{encode_batch, SyncBatch, SyncBatchEnvelope, SyncLearning};
        let learning = SyncLearning {
            training_candidate_id: 1,
            user_id: "default".to_owned(),
            utterance_id: format!("u-{digest}"),
            text_session_id: format!("s-{digest}"),
            audio_object_key: format!("k-{digest}"),
            audio_digest: digest.to_owned(),
            raw_transcript: raw.to_owned(),
            corrected_transcript: corrected.to_owned(),
            source_label: "ime".to_owned(),
            trust_score_bps: 10_000,
        };
        let mut audio = std::collections::BTreeMap::new();
        audio.insert(digest.to_owned(), b"opus-bytes".to_vec());
        let envelope = SyncBatchEnvelope::new(
            SyncBatch {
                device_id: "pixel".to_owned(),
                batch_id: "batch-live".to_owned(),
                learnings: vec![learning],
            },
            audio,
        );
        encode_batch(&envelope).expect("encode")
    }

    /// One blocking bearer-authenticated `POST /v1/sync` with a binary batch
    /// body — the ingest request a paired phone sends. Dependency-free like
    /// [`http_get`].
    fn http_post_sync(addr: SocketAddr, token: &str, body: &[u8]) -> String {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        write!(
            stream,
            "POST /v1/sync HTTP/1.1\r\nHost: idiolect-test\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .expect("write headers");
        stream.write_all(body).expect("write body");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    }

    #[test]
    fn a_running_hosts_ingest_writes_encrypted_history_at_rest() {
        // THE regression pin for the plaintext-ingest P2: through the RUNNING
        // host — SyncHost::start's own ingest store, not one a test built —
        // a phone's POST /v1/sync must land its correction as ciphertext.
        // This guards the exact line that was wrong (the SECOND open inside
        // `start`): with only the first open ciphered, every other test in
        // this crate stays green (proven by mutation during review).
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("test.db");
        let key = dir.path().join("history.key");
        std::fs::write(&key, [7_u8; 32]).expect("the daemon pre-created its key");
        let cfg = SyncHostConfig {
            bind: "127.0.0.1:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: db.clone(),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
            history_key: Some(key.clone()),
        };
        let host = SyncHost::start(cfg, rt.handle()).expect("start");
        let token = host.issue_test_token("pixel").expect("token");

        let response = http_post_sync(
            host.local_addr(),
            &token,
            &sync_batch_bytes("digest-live", "restart trafic", "restart traffic"),
        );
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "the ingest POST must succeed, got: {}",
            response.lines().next().unwrap_or_default()
        );

        // At rest (cipher-less view): not the plaintext. The ciphertext is
        // lowercase hex, which cannot even contain the letters of "restart".
        let plain = open_store(&db, None).expect("reopen plain");
        let stored = plain.recent_history(10).expect("history")[0].text.clone();
        assert_ne!(stored, "restart traffic");
        assert!(!stored.contains("restart"));

        // The daemon's own view (same key) round-trips the correction.
        let daemon_view = open_store(&db, Some(&key)).expect("reopen ciphered");
        assert_eq!(
            daemon_view.recent_history(10).expect("history")[0].text,
            "restart traffic"
        );
    }

    fn test_host(dir: &std::path::Path) -> SyncHost {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let cfg = SyncHostConfig {
            bind: "0.0.0.0:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.join("test.db"),
            audio_root: dir.join("audio"),
            model_path: dir.join("model.bin"),
            tokens_path: dir.join("tokens.json"),
            history_key: None,
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
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
            history_key: None,
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

    /// One blocking `POST /v1/pair` claiming `code` — the request a phone that scanned
    /// the QR would send. Dependency-free like [`http_get`].
    fn http_post_pair(addr: SocketAddr, code: &str) -> String {
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

    /// One blocking bearer-authenticated GET — the request a PAIRED phone sends to the
    /// model routes. Dependency-free like [`http_get`].
    fn http_get_authed(addr: SocketAddr, path: &str, token: &str) -> String {
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
    fn a_paired_device_downloads_the_model_the_host_is_configured_to_serve() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let model_bytes = b"pretend-ggml-model-bytes";
        std::fs::write(dir.path().join("model.bin"), model_bytes).expect("write model");
        let cfg = SyncHostConfig {
            bind: "127.0.0.1:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
            history_key: None,
        };
        let host = SyncHost::start(cfg, rt.handle()).expect("start");
        let token = host.issue_test_token("pixel").expect("token");

        // The manifest must describe the configured model FILE — not 500 on some
        // path the host derived by convention (this is the phone's onboarding call).
        let manifest = http_get_authed(host.local_addr(), "/v1/model/manifest", &token);
        assert!(
            manifest.starts_with("HTTP/1.1 200"),
            "an authed manifest for an existing model must succeed, got: {}",
            manifest.lines().next().unwrap_or_default()
        );
        let expected_sha =
            idiolect_common::digest::file_sha256_hex(dir.path().join("model.bin")).expect("digest");
        assert!(
            manifest.contains(&expected_sha),
            "the manifest must carry the served file's digest"
        );
        assert!(
            manifest.contains(&format!(r#""size":{}"#, model_bytes.len())),
            "the manifest must carry the served file's size"
        );

        let download = http_get_authed(host.local_addr(), "/v1/model", &token);
        assert!(
            download.starts_with("HTTP/1.1 200"),
            "an authed model download must succeed, got: {}",
            download.lines().next().unwrap_or_default()
        );
        assert!(
            download.ends_with(std::str::from_utf8(model_bytes).expect("ascii fixture")),
            "the download must be byte-identical to the file at the configured model path"
        );
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
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
            history_key: None,
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
    fn disabling_the_host_invalidates_the_outstanding_pairing_offer() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = SyncHostConfig {
            bind: "127.0.0.1:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
            history_key: None,
        };
        let host = SyncHost::start(cfg, rt.handle()).expect("start");
        let offer = host.mint_pairing(None).expect("mint");

        // pair → disable → re-enable, all within the code's 10-minute TTL. Disable
        // hides the offer from the dashboard (and the 503 gate blocks redemption),
        // so the code must die at the host — not lie in wait, invisible and
        // uncancellable, for the routes to reopen.
        host.set_enabled(false);
        host.set_enabled(true);

        let response = http_post_pair(host.local_addr(), &offer.code);
        assert!(
            response.starts_with("HTTP/1.1 401"),
            "an offer hidden by disable must not redeem after re-enable, got: {}",
            response.lines().next().unwrap_or_default()
        );
        assert!(
            host.paired_devices().is_empty(),
            "no device may pair with the invalidated code"
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
    fn cancelling_the_pairing_offer_kills_the_code_while_routes_stay_open() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = SyncHostConfig {
            bind: "127.0.0.1:0".parse().expect("addr"),
            pair_url: String::new(),
            tls: false,
            db_path: dir.path().join("test.db"),
            audio_root: dir.path().join("audio"),
            model_path: dir.path().join("model.bin"),
            tokens_path: dir.path().join("tokens.json"),
            history_key: None,
        };
        let host = SyncHost::start(cfg, rt.handle()).expect("start");
        let offer = host.mint_pairing(None).expect("mint");

        host.cancel_pairing();

        let response = http_post_pair(host.local_addr(), &offer.code);
        assert!(
            response.starts_with("HTTP/1.1 401"),
            "a cancelled code must be refused, got: {}",
            response.lines().next().unwrap_or_default()
        );
        // Cancel is not disable: the host must keep serving phone routes.
        assert!(
            !http_get(host.local_addr(), "/v1/model/manifest").starts_with("HTTP/1.1 503"),
            "cancelling a pairing offer must not gate the host"
        );
        assert!(
            host.paired_devices().is_empty(),
            "no device may pair with the cancelled code"
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
