//! The personal **model server**: serves the user's trained/base Whisper `.bin` to
//! the Android app over HTTP (the M5 `GET /model` hop), so the phone can pull an
//! improved model after the PC trains. Bearer-token authenticated and **Range-aware**
//! so an interrupted download resumes.
//!
//! Desktop-only: `axum` is never linked into the Android `.so` (this crate is not a
//! dependency of `idiolect-ffi`). The router carries all the logic and is gate-tested
//! deterministically via `tower::ServiceExt::oneshot`; [`serve`] and the binary are
//! the thin socket glue.
//!
//! For a personal single-user server the model file is read per request (and hashed
//! per manifest request); that is intentionally simple — concurrency is ~one phone.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::device_tokens::{authenticate, DeviceTokenStore};

/// What model the server serves and the per-device token store that guards it (shared
/// with the ingest server, so one device's token authenticates both endpoints).
#[derive(Debug, Clone)]
pub struct ModelServerConfig {
    /// Absolute path to the model `.bin` to serve.
    pub model_path: PathBuf,
    /// Stable identifier the client records (e.g. `base.en` or a personalised id).
    pub model_id: String,
    /// The per-device bearer tokens the client must present one of (S3).
    pub tokens: Arc<DeviceTokenStore>,
}

/// The served model's identity + integrity metadata, so the client knows what to
/// expect and can verify the download (`sha256` is the same lowercase-hex digest the
/// device re-checks at every load).
#[derive(Debug, Serialize)]
pub struct ModelManifest {
    pub id: String,
    pub sha256: String,
    pub size: u64,
}

/// Build the model router: `GET /v1/model/manifest` (identity + digest + size) and
/// `GET /v1/model` (the bytes, Range-aware). Both are bearer-guarded.
pub fn model_router(config: Arc<ModelServerConfig>) -> Router {
    Router::new()
        .route("/v1/model/manifest", get(manifest))
        .route("/v1/model", get(download))
        .with_state(config)
}

/// Bind the model router to `listener` and serve until the process ends.
pub async fn serve(
    listener: tokio::net::TcpListener,
    config: Arc<ModelServerConfig>,
) -> std::io::Result<()> {
    axum::serve(listener, model_router(config)).await
}

async fn manifest(State(config): State<Arc<ModelServerConfig>>, headers: HeaderMap) -> Response {
    if authenticate(&headers, &config.tokens).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let size = match std::fs::metadata(&config.model_path) {
        Ok(meta) => meta.len(),
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let sha256 = match idiolect_common::digest::file_sha256_hex(&config.model_path) {
        Ok(digest) => digest,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    Json(ModelManifest {
        id: config.model_id.clone(),
        sha256,
        size,
    })
    .into_response()
}

async fn download(State(config): State<Arc<ModelServerConfig>>, headers: HeaderMap) -> Response {
    if authenticate(&headers, &config.tokens).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let bytes = match std::fs::read(&config.model_path) {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let total = bytes.len() as u64;
    match headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        Some(range) => serve_range(&bytes, total, range),
        None => octet_response(StatusCode::OK, bytes, None),
    }
}

/// Serve a single `bytes=START-[END]` range as `206 Partial Content` (the resume path).
fn serve_range(bytes: &[u8], total: u64, range: &str) -> Response {
    let unsatisfiable = || {
        (
            StatusCode::RANGE_NOT_SATISFIABLE,
            [(header::CONTENT_RANGE, format!("bytes */{total}"))],
        )
            .into_response()
    };
    let Some((start, end)) = parse_single_range(range, total) else {
        return unsatisfiable();
    };
    let slice = bytes[start as usize..=end as usize].to_vec();
    octet_response(
        StatusCode::PARTIAL_CONTENT,
        slice,
        Some((start, end, total)),
    )
}

/// Parse a single closed range from `bytes=START-[END]`, returning inclusive
/// `(start, end)` or `None` if malformed or out of bounds.
fn parse_single_range(range: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let spec = range.strip_prefix("bytes=")?;
    let (start_str, end_str) = spec.split_once('-')?;
    let start: u64 = start_str.parse().ok()?;
    let end: u64 = if end_str.is_empty() {
        total - 1
    } else {
        end_str.parse().ok()?
    };
    (start <= end && end < total).then_some((start, end))
}

/// An `application/octet-stream` body with `Accept-Ranges`, plus a `Content-Range`
/// when this is a partial response.
fn octet_response(
    status: StatusCode,
    body: Vec<u8>,
    content_range: Option<(u64, u64, u64)>,
) -> Response {
    let len = body.len() as u64;
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    if let Some((start, end, total)) = content_range {
        if let Ok(value) = HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")) {
            headers.insert(header::CONTENT_RANGE, value);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TOKEN: &str = "test-token";

    /// A token store with the known `TOKEN` bound, so tests keep asserting against it.
    fn tokens(dir: &std::path::Path) -> Arc<DeviceTokenStore> {
        let mut store = DeviceTokenStore::open(dir.join("tokens.json")).expect("tokens");
        store.bind(TOKEN, "pixel", "default").expect("bind token");
        Arc::new(store)
    }

    fn fixture() -> (tempfile::TempDir, Arc<ModelServerConfig>, Vec<u8>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("model.bin");
        let bytes: Vec<u8> = (0..1000_u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &bytes).expect("write model");
        let config = Arc::new(ModelServerConfig {
            model_path: path,
            model_id: "base.en".to_owned(),
            tokens: tokens(dir.path()),
        });
        (dir, config, bytes)
    }

    fn request(uri: &str, auth: Option<&str>, range: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(uri);
        if let Some(token) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(range) = range {
            builder = builder.header(header::RANGE, range);
        }
        builder.body(Body::empty()).expect("request")
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes()
            .to_vec()
    }

    #[tokio::test]
    async fn both_endpoints_require_a_valid_bearer_token() {
        let (_dir, config, _) = fixture();
        for (uri, auth) in [("/v1/model", None), ("/v1/model", Some("wrong"))] {
            let response = model_router(config.clone())
                .oneshot(request(uri, auth, None))
                .await
                .expect("router");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let response = model_router(config)
            .oneshot(request("/v1/model/manifest", None, None))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn manifest_reports_id_size_and_digest() {
        let (_dir, config, bytes) = fixture();
        let response = model_router(config)
            .oneshot(request("/v1/model/manifest", Some(TOKEN), None))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::OK);
        let json: serde_json::Value =
            serde_json::from_slice(&body_bytes(response).await).expect("json");
        assert_eq!(json["id"], "base.en");
        assert_eq!(json["size"].as_u64().unwrap(), bytes.len() as u64);
        // The advertised digest is the canonical content digest the device re-checks.
        assert_eq!(
            json["sha256"],
            idiolect_common::digest::audio_sha256_hex(&bytes).as_str()
        );
    }

    #[tokio::test]
    async fn a_full_download_returns_all_bytes_and_advertises_ranges() {
        let (_dir, config, bytes) = fixture();
        let response = model_router(config)
            .oneshot(request("/v1/model", Some(TOKEN), None))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes"
        );
        assert_eq!(body_bytes(response).await, bytes);
    }

    #[tokio::test]
    async fn a_range_request_resumes_from_the_offset() {
        let (_dir, config, bytes) = fixture();
        let response = model_router(config)
            .oneshot(request("/v1/model", Some(TOKEN), Some("bytes=400-")))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            &format!("bytes 400-{}/{}", bytes.len() - 1, bytes.len())
        );
        assert_eq!(body_bytes(response).await, bytes[400..]);
    }

    #[tokio::test]
    async fn a_bounded_range_returns_exactly_that_slice() {
        let (_dir, config, bytes) = fixture();
        let response = model_router(config)
            .oneshot(request("/v1/model", Some(TOKEN), Some("bytes=10-19")))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(body_bytes(response).await, bytes[10..=19]);
    }

    #[tokio::test]
    async fn an_out_of_bounds_range_is_rejected() {
        let (_dir, config, bytes) = fixture();
        let response = model_router(config)
            .oneshot(request(
                "/v1/model",
                Some(TOKEN),
                Some(&format!("bytes={}-", bytes.len() + 10)),
            ))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }

    #[tokio::test]
    async fn a_missing_model_file_is_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = Arc::new(ModelServerConfig {
            model_path: dir.path().join("absent.bin"),
            model_id: "base.en".to_owned(),
            tokens: tokens(dir.path()),
        });
        let response = model_router(config)
            .oneshot(request("/v1/model", Some(TOKEN), None))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
