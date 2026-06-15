//! The personal model/sync server binary. It serves the model (`GET /model`, M5)
//! and — when a local store + audio root are configured — the learning ingest
//! endpoint (`POST /v1/sync`, M6) on the same port, sharing the bearer token.
//! Configured from the environment:
//!
//!   IDIOLECT_MODEL_PATH   the model `.bin` to serve (required)
//!   IDIOLECT_SYNC_TOKEN   bearer token the client must present (required)
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
use idiolect_sync_server::ingest_server::{ingest_router, IngestServerState};
use idiolect_sync_server::model_server::{model_router, ModelServerConfig};

#[tokio::main]
async fn main() {
    let config = match config_from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("idiolect-sync-server: {message}");
            std::process::exit(2);
        }
    };
    let model_id = config.model_id.clone();
    let token = config.bearer_token.clone();
    let mut app = model_router(Arc::new(config));

    // The M6 ingest half is opt-in: enabled only when the local store + audio root
    // are configured, so the M5 model-only deployment keeps working unchanged.
    let ingest_enabled = match ingest_state_from_env(&token) {
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

fn config_from_env() -> Result<ModelServerConfig, String> {
    let model_path = std::env::var("IDIOLECT_MODEL_PATH")
        .map_err(|_| "set IDIOLECT_MODEL_PATH to the model .bin to serve".to_owned())?;
    let bearer_token = std::env::var("IDIOLECT_SYNC_TOKEN")
        .map_err(|_| "set IDIOLECT_SYNC_TOKEN to the bearer token".to_owned())?;
    let model_id = std::env::var("IDIOLECT_MODEL_ID").unwrap_or_else(|_| "base.en".to_owned());
    Ok(ModelServerConfig {
        model_path: PathBuf::from(model_path),
        model_id,
        bearer_token,
    })
}

/// Build the ingest state when both the DB and audio root are configured. Returns
/// `Ok(None)` when neither is set (the model-only server); errors if only one is set
/// or the store can't be opened/migrated.
fn ingest_state_from_env(bearer_token: &str) -> Result<Option<IngestServerState>, String> {
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
            Ok(Some(IngestServerState::new(
                store,
                audio_store,
                bearer_token.to_owned(),
            )))
        }
        _ => Err(
            "set BOTH IDIOLECT_DB_PATH and IDIOLECT_AUDIO_ROOT to enable sync ingest, or neither"
                .to_owned(),
        ),
    }
}
