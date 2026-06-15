//! The personal model/sync server binary. It serves the model (`GET /model`, M5)
//! and — when a local store + audio root are configured — the learning ingest
//! endpoint (`POST /v1/sync`, M6) on the same port, both authenticating against one
//! shared per-device token store (S3). Configured from the environment:
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
//!
//! Point `IDIOLECT_DB_PATH`/`IDIOLECT_AUDIO_ROOT` at the same db + audio root you
//! pass `trainerctl --db/--audio-root`, so ingested learnings land where the trainer
//! reads them. The decoded cache is the `decoded-cache` sibling of the audio root,
//! matching `trainerctl`.

use std::path::PathBuf;
use std::sync::Arc;

use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_sync_server::device_tokens::DeviceTokenStore;
use idiolect_sync_server::ingest_server::{ingest_router, IngestServerState};
use idiolect_sync_server::model_server::{model_router, ModelServerConfig};

#[tokio::main]
async fn main() {
    let settings = match settings_from_env() {
        Ok(settings) => settings,
        Err(message) => {
            eprintln!("idiolect-sync-server: {message}");
            std::process::exit(2);
        }
    };

    // One shared per-device token store guards both the model and ingest endpoints (S3).
    let tokens = match open_tokens(&settings) {
        Ok(tokens) => Arc::new(tokens),
        Err(message) => {
            eprintln!("idiolect-sync-server: {message}");
            std::process::exit(2);
        }
    };
    if tokens.device_count() == 0 {
        eprintln!(
            "idiolect-sync-server: WARNING no devices paired — set IDIOLECT_SYNC_TOKEN or \
             pair a device; all requests will be rejected until then"
        );
    }

    let model_id = settings.model_id.clone();
    let mut app = model_router(Arc::new(ModelServerConfig {
        model_path: settings.model_path.clone(),
        model_id: settings.model_id.clone(),
        tokens: Arc::clone(&tokens),
    }));

    // The M6 ingest half is opt-in: enabled only when the local store + audio root
    // are configured, so the M5 model-only deployment keeps working unchanged.
    let ingest_enabled = match ingest_state_from_env(Arc::clone(&tokens)) {
        Ok(Some(state)) => {
            app = app.merge(ingest_router(Arc::new(state)));
            true
        }
        Ok(None) => false,
        Err(message) => {
            eprintln!("idiolect-sync-server: {message}");
            std::process::exit(2);
        }
    };

    let addr = std::env::var("IDIOLECT_SYNC_ADDR").unwrap_or_else(|_| "127.0.0.1:8765".to_owned());
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("idiolect-sync-server: bind {addr}: {error}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "idiolect-sync-server: serving model '{model_id}' on {addr} (ingest: {})",
        if ingest_enabled { "on" } else { "off" }
    );
    if let Err(error) = axum::serve(listener, app).await {
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
    let path = settings.tokens_path.clone().unwrap_or_else(|| {
        match std::env::var("IDIOLECT_DB_PATH") {
            Ok(db) => PathBuf::from(db),
            Err(_) => settings.model_path.clone(),
        }
        .with_file_name("device-tokens.json")
    });
    let mut tokens = DeviceTokenStore::open(&path)
        .map_err(|error| format!("open token store {}: {error}", path.display()))?;
    if let Some(token) = &settings.sync_token {
        tokens
            .bind(token, "default-device", "default")
            .map_err(|error| format!("bind configured token: {error}"))?;
    }
    Ok(tokens)
}

/// Build the ingest state when both the DB and audio root are configured. Returns
/// `Ok(None)` when neither is set (the model-only server); errors if only one is set
/// or the store can't be opened/migrated.
fn ingest_state_from_env(
    tokens: Arc<DeviceTokenStore>,
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
