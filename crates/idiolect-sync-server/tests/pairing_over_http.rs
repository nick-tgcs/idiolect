//! End-to-end pairing over a *real* TCP socket — the contract the Android emulator
//! depends on. The unit/handler tests drive the routers in-process via
//! `tower::oneshot`; this stands up the **composed** server ([`build_app`], the exact
//! app the `idiolect-sync-server` binary serves) on an ephemeral port and pairs over
//! HTTP/1.1 the same way the phone does: `POST /v1/pair` with the live code earns a
//! per-device token, and that token then authenticates `GET /v1/model/manifest`. A
//! wrong code and a missing token are both rejected over the wire.
//!
//! This is the seam the emulator e2e mirrors: emulator → `10.0.2.2:<port>` → this app.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use idiolect_sync_server::build_app;
use idiolect_sync_server::device_tokens::DeviceTokenStore;
use idiolect_sync_server::model_server::ModelServerConfig;
use idiolect_sync_server::pairing::{system_now, PairingServerState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Boot the composed app on `127.0.0.1:0`. Returns its address, the one live pairing
/// code, and the `TempDir` backing the model + token store (held by the caller so the
/// files outlive the server).
async fn spawn_server() -> (SocketAddr, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let model_path = dir.path().join("model.bin");
    std::fs::write(
        &model_path,
        b"ggml-fake-model-bytes-for-the-manifest-digest",
    )
    .expect("model");
    let tokens_path = dir.path().join("device-tokens.json");
    let tokens = Arc::new(Mutex::new(
        DeviceTokenStore::open(&tokens_path).expect("token store"),
    ));

    // Mint the code against the *same* pairing state the route serves, exactly as the
    // binary's `--pair` does — so the code the test "types" is the one the POST matches.
    let pairing = Arc::new(PairingServerState::new(Arc::clone(&tokens)));
    let code = pairing.generate_code(system_now());

    let model = Arc::new(ModelServerConfig {
        model_path,
        model_id: "test.en".to_owned(),
        tokens: Arc::clone(&tokens),
    });
    let app = build_app(model, Arc::clone(&pairing), None);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (addr, code, dir)
}

/// Drive one HTTP/1.1 request over a fresh connection and read the whole response.
/// `Connection: close` lets the server end the body by closing, so `read_to_end`
/// terminates without us parsing `Content-Length`. Returns `(status_code, body)`.
async fn send(addr: SocketAddr, request: String) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    stream.flush().await.expect("flush");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read response");
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or("")
        .to_owned();
    (status, body)
}

fn post(path: &str, host: SocketAddr, body: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Content-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len(),
    )
}

fn get(path: &str, host: SocketAddr, bearer: Option<&str>) -> String {
    let auth = bearer
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{auth}Connection: close\r\n\r\n")
}

/// Pull the string value of a flat JSON `"key":"value"` out of a compact body.
fn json_field<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":\"");
    let start = body.find(&needle)? + needle.len();
    let rest = &body[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

#[tokio::test]
async fn a_device_pairs_over_real_http_then_fetches_the_manifest_with_its_token() {
    let (addr, code, _dir) = spawn_server().await;

    // Redeem the live code over a real socket → 201 Created + a per-device token.
    let body = format!(r#"{{"code":"{code}","device_id":"emulator-5554"}}"#);
    let (status, resp) = send(addr, post("/v1/pair", addr, &body)).await;
    assert_eq!(status, 201, "pair response body: {resp}");
    assert_eq!(json_field(&resp, "device_id"), Some("emulator-5554"));
    let token = json_field(&resp, "token")
        .expect("token in pair response")
        .to_owned();
    assert!(!token.is_empty(), "issued token is empty");

    // The freshly issued token authenticates the model endpoint over the same wire.
    let (status, manifest) = send(addr, get("/v1/model/manifest", addr, Some(&token))).await;
    assert_eq!(status, 200, "manifest response body: {manifest}");
    assert!(
        manifest.contains("test.en"),
        "manifest should carry the model id: {manifest}"
    );
}

#[tokio::test]
async fn the_manifest_is_unauthorized_without_a_paired_token() {
    let (addr, _code, _dir) = spawn_server().await;

    let (status, _) = send(addr, get("/v1/model/manifest", addr, None)).await;
    assert_eq!(status, 401, "no token must be rejected");

    let (status, _) = send(
        addr,
        get("/v1/model/manifest", addr, Some("not-a-real-token")),
    )
    .await;
    assert_eq!(status, 401, "a forged token must be rejected");
}

#[tokio::test]
async fn a_wrong_code_is_rejected_over_real_http_and_issues_no_token() {
    let (addr, _code, _dir) = spawn_server().await;

    let body = r#"{"code":"WRONGCOD","device_id":"emulator-5554"}"#;
    let (status, resp) = send(addr, post("/v1/pair", addr, body)).await;
    assert_eq!(status, 401, "a wrong code must not pair: {resp}");

    // And no token leaked: the manifest stays locked.
    let (status, _) = send(addr, get("/v1/model/manifest", addr, Some("WRONGCOD"))).await;
    assert_eq!(status, 401);
}
