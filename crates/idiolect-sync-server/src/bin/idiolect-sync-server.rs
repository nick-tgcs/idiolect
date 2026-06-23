//! The personal model/sync server binary. It serves the model (`GET /model`, M5),
//! the device-pairing handshake (`POST /v1/pair`, S3) and — when a local store +
//! audio root are configured — the learning ingest endpoint (`POST /v1/sync`, M6) on
//! the same port. The model and ingest endpoints authenticate against one shared
//! per-device token store (S3); pairing issues the tokens. Configured from the
//! environment:
//!
//!   IDIOLECT_MODEL_PATH   the model `.bin` to serve (required)
//!   IDIOLECT_SYNC_TOKEN   a shared bearer token bound to a "default-device" on start
//!                         (optional; the pairing handshake issues per-device tokens)
//!   IDIOLECT_TOKENS_PATH  the device-token store file (default: device-tokens.json
//!                         beside the DB if configured, else beside the model)
//!   IDIOLECT_MODEL_ID     advertised model id (default: base.en)
//!   IDIOLECT_SYNC_ADDR    bind address (default: 127.0.0.1:8765)
//!   IDIOLECT_DB_PATH      local metadata store; enables ingest (with AUDIO_ROOT)
//!   IDIOLECT_AUDIO_ROOT   source-audio root; enables ingest (with DB_PATH)
//!   IDIOLECT_PAIR_URL     externally-reachable base URL embedded in the pairing QR
//!                         (default: https://<IDIOLECT_SYNC_ADDR>, or http with --no-tls);
//!                         set this to the tailnet address the phone can actually reach, not
//!                         the loopback bind
//!
//! TLS is the default. The server presents a persisted self-signed cert (the
//! `sync-tls-cert.pem` and `sync-tls-key.pem` files beside the token store) and the pairing
//! QR carries that cert's SPKI fingerprint, which the phone pins on pairing — trust on first
//! pairing, no CA, defeating a LAN man-in-the-middle. Pass `--no-tls` to serve cleartext
//! instead, for deployments already inside an encrypted tunnel (a tailnet or a localhost
//! onion hop) where TLS is redundant.
//!
//! Pass `--pair` to mint a one-time pairing code and print it as a scannable QR — plus the
//! grouped code, URL, and (under TLS) the cert pin to type/verify by hand — to stdout, valid
//! 10 minutes. A new device scans or enters it and redeems at `POST /v1/pair` to earn its own
//! bearer token. The code lives only in memory, so a restart invalidates it (re-run `--pair`).
//!
//! Point `IDIOLECT_DB_PATH`/`IDIOLECT_AUDIO_ROOT` at the same db + audio root you
//! pass `trainerctl --db/--audio-root`, so ingested learnings land where the trainer
//! reads them. The decoded cache is the `decoded-cache` sibling of the audio root,
//! matching `trainerctl`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_sync_server::build_app;
use idiolect_sync_server::device_tokens::DeviceTokenStore;
use idiolect_sync_server::ingest_server::IngestServerState;
use idiolect_sync_server::model_server::ModelServerConfig;
use idiolect_sync_server::pairing::{system_now, PairingServerState};
use idiolect_sync_server::pairing_qr::pairing_announcement;
use idiolect_sync_server::tls::{serve_tls, ServerTls};

#[tokio::main]
async fn main() {
    let settings = match settings_from_env() {
        Ok(settings) => settings,
        Err(message) => {
            eprintln!("idiolect-sync-server: {message}");
            std::process::exit(2);
        }
    };

    // One shared per-device token store guards the model and ingest endpoints; the
    // pairing endpoint mutates it, so it lives behind a Mutex (S3).
    let tokens = match open_tokens(&settings) {
        Ok(tokens) => Arc::new(Mutex::new(tokens)),
        Err(message) => {
            eprintln!("idiolect-sync-server: {message}");
            std::process::exit(2);
        }
    };

    let addr = std::env::var("IDIOLECT_SYNC_ADDR").unwrap_or_else(|_| "127.0.0.1:8765".to_owned());

    // TLS is the default: the server presents a persisted self-signed cert and the phone
    // pins its SPKI fingerprint on pairing (the `&f=` the QR carries). `--no-tls` drops to
    // cleartext, for deployments already inside an encrypted tunnel — a tailnet or a
    // localhost onion hop — where TLS is redundant.
    let no_tls = std::env::args().skip(1).any(|arg| arg == "--no-tls");
    let tls = if no_tls {
        None
    } else {
        match ServerTls::load_or_generate(&tls_dir(&settings)) {
            Ok(tls) => Some(tls),
            Err(error) => {
                eprintln!("idiolect-sync-server: tls setup: {error}");
                std::process::exit(2);
            }
        }
    };

    // `--pair` mints a one-time code against the live, in-memory pairing state the
    // `/v1/pair` route serves, so the code the operator reads is exactly the one the
    // device's POST will match. It is shown as a scannable QR — carrying the base URL, the
    // code, and (under TLS) the cert pin, so one scan both pairs and pins — plus the
    // typed-by-hand fallback.
    let pair_mode = std::env::args().skip(1).any(|arg| arg == "--pair");
    let pairing = Arc::new(PairingServerState::new(Arc::clone(&tokens)));
    if pair_mode {
        let code = pairing.generate_code(system_now());
        let base_url = pair_base_url(
            std::env::var("IDIOLECT_PAIR_URL").ok(),
            &addr,
            tls.is_some(),
        );
        print!(
            "{}",
            pairing_announcement(&base_url, &code, tls.as_ref().map(ServerTls::fingerprint))
        );
        eprintln!(
            "idiolect-sync-server: pairing code valid for 10 minutes — scan the QR or enter it on the device"
        );
    }

    let paired_devices = tokens
        .lock()
        .expect("token store mutex poisoned")
        .device_count();
    if paired_devices == 0 && !pair_mode {
        eprintln!(
            "idiolect-sync-server: WARNING no devices paired — re-run with --pair to mint a \
             pairing code (or set IDIOLECT_SYNC_TOKEN); all requests are rejected until then"
        );
    }

    let model_id = settings.model_id.clone();
    let model = Arc::new(ModelServerConfig {
        model_path: settings.model_path.clone(),
        model_id: settings.model_id.clone(),
        tokens: Arc::clone(&tokens),
    });

    // The M6 ingest half is opt-in: enabled only when the local store + audio root
    // are configured, so the M5 model-only deployment keeps working unchanged.
    let ingest = match ingest_state_from_env(Arc::clone(&tokens)) {
        Ok(state) => state.map(Arc::new),
        Err(message) => {
            eprintln!("idiolect-sync-server: {message}");
            std::process::exit(2);
        }
    };
    let ingest_enabled = ingest.is_some();

    // One composed app — the same `build_app` the integration test stands up on a socket.
    let app = build_app(model, Arc::clone(&pairing), ingest);

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("idiolect-sync-server: bind {addr}: {error}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "idiolect-sync-server: serving model '{model_id}' on {addr} ({}, ingest: {})",
        if tls.is_some() { "https" } else { "http" },
        if ingest_enabled { "on" } else { "off" }
    );

    // The default secure path terminates TLS with the persisted self-signed cert; `--no-tls`
    // serves the same composed app in cleartext for already-tunnelled deployments.
    let served = match tls {
        Some(tls) => match tls.acceptor() {
            Ok(acceptor) => serve_tls(listener, acceptor, app).await,
            Err(error) => {
                eprintln!("idiolect-sync-server: tls acceptor: {error}");
                std::process::exit(2);
            }
        },
        None => axum::serve(listener, app).await,
    };
    if let Err(error) = served {
        eprintln!("idiolect-sync-server: {error}");
        std::process::exit(1);
    }
}

/// The environment-derived server settings (token wiring is resolved separately).
struct Settings {
    model_path: PathBuf,
    model_id: String,
    sync_token: Option<String>,
    tokens_path: Option<PathBuf>,
}

fn settings_from_env() -> Result<Settings, String> {
    let model_path = std::env::var("IDIOLECT_MODEL_PATH")
        .map_err(|_| "set IDIOLECT_MODEL_PATH to the model .bin to serve".to_owned())?;
    let model_id = std::env::var("IDIOLECT_MODEL_ID").unwrap_or_else(|_| "base.en".to_owned());
    Ok(Settings {
        model_path: PathBuf::from(model_path),
        model_id,
        sync_token: std::env::var("IDIOLECT_SYNC_TOKEN").ok(),
        tokens_path: std::env::var("IDIOLECT_TOKENS_PATH")
            .ok()
            .map(PathBuf::from),
    })
}

/// Open the persisted device-token store and, if `IDIOLECT_SYNC_TOKEN` is configured,
/// (re)bind it to a `default-device` so a single-secret deployment keeps working until a
/// device is paired. The store lives beside the DB if one is configured, else beside the
/// model — wherever it can persist across restarts.
fn open_tokens(settings: &Settings) -> Result<DeviceTokenStore, String> {
    let path = tokens_path(settings);
    let mut tokens = DeviceTokenStore::open(&path)
        .map_err(|error| format!("open token store {}: {error}", path.display()))?;
    if let Some(token) = &settings.sync_token {
        tokens
            .bind(token, "default-device", "default")
            .map_err(|error| format!("bind configured token: {error}"))?;
    }
    Ok(tokens)
}

/// Where the device-token store lives: an explicit `IDIOLECT_TOKENS_PATH`, else
/// `device-tokens.json` beside the DB (if configured) or the model. The TLS cert + key are
/// persisted in the same directory, so the server's pinned identity travels with its tokens.
fn tokens_path(settings: &Settings) -> PathBuf {
    settings.tokens_path.clone().unwrap_or_else(|| {
        match std::env::var("IDIOLECT_DB_PATH") {
            Ok(db) => PathBuf::from(db),
            Err(_) => settings.model_path.clone(),
        }
        .with_file_name("device-tokens.json")
    })
}

/// The directory the persisted self-signed cert + key live in: alongside the token store.
fn tls_dir(settings: &Settings) -> PathBuf {
    tokens_path(settings)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Build the ingest state when both the DB and audio root are configured. Returns
/// `Ok(None)` when neither is set (the model-only server); errors if only one is set
/// or the store can't be opened/migrated.
fn ingest_state_from_env(
    tokens: Arc<Mutex<DeviceTokenStore>>,
) -> Result<Option<IngestServerState>, String> {
    match (
        std::env::var("IDIOLECT_DB_PATH").ok(),
        std::env::var("IDIOLECT_AUDIO_ROOT").ok(),
    ) {
        (None, None) => Ok(None),
        (Some(db), Some(audio_root)) => {
            let mut store = SqliteMetadataStore::open_path(&db)
                .map_err(|error| format!("open store {db}: {error}"))?;
            store
                .migrate()
                .map_err(|error| format!("migrate store: {error}"))?;
            let audio_root = PathBuf::from(audio_root);
            let decoded_cache = audio_root.with_file_name("decoded-cache");
            let audio_store = FileAudioStore::new(audio_root, decoded_cache);
            Ok(Some(IngestServerState::new(store, audio_store, tokens)))
        }
        _ => Err(
            "set BOTH IDIOLECT_DB_PATH and IDIOLECT_AUDIO_ROOT to enable sync ingest, or neither"
                .to_owned(),
        ),
    }
}

/// The base URL to embed in the pairing QR. An explicit `IDIOLECT_PAIR_URL` (the operator's
/// tailnet address the phone can reach) wins, trimmed of a trailing slash — the operator
/// chose its scheme. Otherwise fall back to the bind address, with the scheme following
/// `tls_on`: `https` by default, `http` under `--no-tls`. The fallback is correct for
/// same-host testing but usually not reachable from a phone, so `--pair` documents setting
/// `IDIOLECT_PAIR_URL`.
fn pair_base_url(explicit: Option<String>, bind_addr: &str, tls_on: bool) -> String {
    match explicit {
        Some(url) if !url.trim().is_empty() => url.trim().trim_end_matches('/').to_owned(),
        _ => {
            let scheme = if tls_on { "https" } else { "http" };
            format!("{scheme}://{bind_addr}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pair_base_url;

    #[test]
    fn an_explicit_pair_url_wins_and_is_trimmed() {
        // The operator's explicit URL wins verbatim, scheme and all, regardless of TLS.
        assert_eq!(
            pair_base_url(
                Some("https://100.64.0.7:8765/".to_owned()),
                "127.0.0.1:8765",
                true,
            ),
            "https://100.64.0.7:8765",
        );
    }

    #[test]
    fn the_default_scheme_follows_tls() {
        // No explicit URL: the bind-address fallback is https when TLS is on (the default),
        // http under --no-tls.
        assert_eq!(
            pair_base_url(None, "0.0.0.0:8765", true),
            "https://0.0.0.0:8765",
        );
        assert_eq!(
            pair_base_url(None, "0.0.0.0:8765", false),
            "http://0.0.0.0:8765",
        );
        assert_eq!(
            pair_base_url(Some("   ".to_owned()), "127.0.0.1:8765", true),
            "https://127.0.0.1:8765",
        );
    }
}
