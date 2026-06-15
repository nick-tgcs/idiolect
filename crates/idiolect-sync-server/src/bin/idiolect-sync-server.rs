//! The personal model/sync server binary. v1 serves the model (`GET /model`, M5);
//! the sync ingest endpoint (POST) lands in M6. Configured from the environment:
//!
//!   IDIOLECT_MODEL_PATH   the model `.bin` to serve (required)
//!   IDIOLECT_SYNC_TOKEN   bearer token the client must present (required)
//!   IDIOLECT_MODEL_ID     advertised model id (default: base.en)
//!   IDIOLECT_SYNC_ADDR    bind address (default: 127.0.0.1:8765)

use std::path::PathBuf;
use std::sync::Arc;

use idiolect_sync_server::model_server::{serve, ModelServerConfig};

#[tokio::main]
async fn main() {
    let config = match config_from_env() {
        Ok(config) => config,
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
        "idiolect-sync-server: serving model '{}' on {addr}",
        config.model_id
    );
    if let Err(error) = serve(listener, Arc::new(config)).await {
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
