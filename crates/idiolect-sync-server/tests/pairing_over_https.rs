//! End-to-end pairing over a *real* TLS socket — the **default** transport, and the
//! contract the Android emulator e2e mirrors. The cleartext sibling
//! (`pairing_over_http.rs`) is the `--no-tls` fallback; this stands up the same composed
//! [`build_app`] over `tokio-rustls` with the server's persisted self-signed cert and pairs
//! the way the phone does — but the client is a **pinning** one: it trusts the server iff
//! the presented leaf cert's DER `SubjectPublicKeyInfo` hashes to the fingerprint the
//! pairing QR carries, exactly as the Android `X509TrustManager` does.
//!
//! The positive path (a correct pin) pairs and authenticates the model endpoint; the
//! negative path proves the pin actually gates the wire: a client pinning the *wrong*
//! fingerprint is refused at the TLS handshake, so the pairing code and the issued token
//! are never transmitted to a man-in-the-middle.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use idiolect_common::digest::sha256_hex;
use idiolect_sync_server::build_app;
use idiolect_sync_server::device_tokens::DeviceTokenStore;
use idiolect_sync_server::model_server::ModelServerConfig;
use idiolect_sync_server::pairing::{system_now, PairingServerState};
use idiolect_sync_server::tls::{ensure_crypto_provider, serve_tls, ServerTls};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

/// Boot the composed app over real TLS on `127.0.0.1:0`. Returns its address, the one live
/// pairing code, the server's SPKI fingerprint (the pin the QR would carry), and the
/// `TempDir` backing the model + token store + persisted cert (held by the caller so the
/// files outlive the server).
async fn spawn_tls_server() -> (SocketAddr, String, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let model_path = dir.path().join("model.bin");
    std::fs::write(
        &model_path,
        b"ggml-fake-model-bytes-for-the-manifest-digest",
    )
    .expect("model");
    let tokens = Arc::new(Mutex::new(
        DeviceTokenStore::open(dir.path().join("device-tokens.json")).expect("token store"),
    ));

    // Mint the code against the same pairing state the route serves, exactly as `--pair` does.
    let pairing = Arc::new(PairingServerState::new(Arc::clone(&tokens)));
    let code = pairing.generate_code(system_now());

    let model = Arc::new(ModelServerConfig {
        model_path,
        model_id: "test.en".to_owned(),
        tokens: Arc::clone(&tokens),
    });
    let app = build_app(model, Arc::clone(&pairing), None);

    let tls = ServerTls::load_or_generate(dir.path()).expect("tls identity");
    let fingerprint = tls.fingerprint().to_owned();
    let acceptor = tls.acceptor().expect("acceptor");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = serve_tls(listener, acceptor, app).await;
    });
    (addr, code, fingerprint, dir)
}

/// A rustls verifier that trusts the server iff the presented leaf cert's DER SPKI hashes to
/// `expected` — the exact pin-on-pairing check the Android `X509TrustManager` performs.
/// Hostname/CA are intentionally ignored: the server is IP-addressed and self-signed, and
/// the out-of-band pin is the whole identity. The handshake *signature* is still verified
/// (delegated to the provider), so a client must prove possession of the pinned key.
#[derive(Debug)]
struct SpkiPin {
    expected: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for SpkiPin {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        use x509_parser::prelude::*;
        let (_, cert) = X509Certificate::from_der(end_entity.as_ref())
            .map_err(|error| rustls::Error::General(format!("parse presented cert: {error}")))?;
        if sha256_hex(cert.public_key().raw) == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "certificate pin mismatch".to_owned(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Open a pinning TLS connection to `addr`, trusting only a server whose SPKI hashes to
/// `pin`. Errors (without ever sending app data) if the presented cert fails the pin.
async fn tls_connect(addr: SocketAddr, pin: &str) -> std::io::Result<TlsStream<TcpStream>> {
    ensure_crypto_provider();
    let provider = rustls::crypto::CryptoProvider::get_default()
        .expect("crypto provider installed")
        .clone();
    let verifier = Arc::new(SpkiPin {
        expected: pin.to_owned(),
        provider,
    });
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    // The pin is the identity, so the SNI name is unchecked; any valid name will do.
    let server_name = ServerName::try_from("idiolect-sync").expect("server name");
    let tcp = TcpStream::connect(addr).await?;
    connector.connect(server_name, tcp).await
}

/// Drive one HTTP/1.1 request over a pinning TLS connection and read the whole response.
/// `Connection: close` lets the server end the body by closing. Panics if the pin does not
/// match (use [`tls_connect`] directly to assert a refusal). Returns `(status_code, body)`.
async fn send_tls(addr: SocketAddr, pin: &str, request: String) -> (u16, String) {
    let mut tls = tls_connect(addr, pin)
        .await
        .expect("tls connect (the pin is expected to match here)");
    tls.write_all(request.as_bytes())
        .await
        .expect("write request");
    tls.flush().await.expect("flush");
    let mut buf = Vec::new();
    tls.read_to_end(&mut buf).await.expect("read response");
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
async fn a_device_pairs_over_real_tls_then_fetches_the_manifest_with_its_token() {
    let (addr, code, pin, _dir) = spawn_tls_server().await;

    // Redeem the live code over a real TLS socket, pinning the server's SPKI → 201 + token.
    let body = format!(r#"{{"code":"{code}","device_id":"emulator-5554"}}"#);
    let (status, resp) = send_tls(addr, &pin, post("/v1/pair", addr, &body)).await;
    assert_eq!(status, 201, "pair over TLS response body: {resp}");
    assert_eq!(json_field(&resp, "device_id"), Some("emulator-5554"));
    let token = json_field(&resp, "token")
        .expect("token in pair response")
        .to_owned();
    assert!(!token.is_empty(), "issued token is empty");

    // The freshly issued token authenticates the model endpoint over the same pinned wire.
    let (status, manifest) =
        send_tls(addr, &pin, get("/v1/model/manifest", addr, Some(&token))).await;
    assert_eq!(status, 200, "manifest response body: {manifest}");
    assert!(
        manifest.contains("test.en"),
        "manifest should carry the model id: {manifest}"
    );
}

#[tokio::test]
async fn the_manifest_is_unauthorized_without_a_paired_token_over_tls() {
    let (addr, _code, pin, _dir) = spawn_tls_server().await;

    let (status, _) = send_tls(addr, &pin, get("/v1/model/manifest", addr, None)).await;
    assert_eq!(status, 401, "no token must be rejected even over TLS");
}

#[tokio::test]
async fn a_wrong_code_is_rejected_over_real_tls_and_issues_no_token() {
    let (addr, _code, pin, _dir) = spawn_tls_server().await;

    let body = r#"{"code":"WRONGCOD","device_id":"emulator-5554"}"#;
    let (status, resp) = send_tls(addr, &pin, post("/v1/pair", addr, body)).await;
    assert_eq!(status, 401, "a wrong code must not pair: {resp}");
}

#[tokio::test]
async fn a_client_pinning_the_wrong_fingerprint_is_refused_at_the_handshake() {
    let (addr, _code, _pin, _dir) = spawn_tls_server().await;

    // A man-in-the-middle (or the real server under a pin the phone never scanned) fails the
    // SPKI check, so the TLS handshake never completes — `connect` errors *before* any
    // request bytes (the pairing code, the bearer token) leave the client. This is the whole
    // point of pin-on-pairing: cleartext-grade exposure is impossible on the default path.
    let wrong_pin = "0".repeat(64);
    let refused = tls_connect(addr, &wrong_pin).await;
    assert!(
        refused.is_err(),
        "a client pinning the wrong fingerprint must abort the handshake, not connect"
    );
}
