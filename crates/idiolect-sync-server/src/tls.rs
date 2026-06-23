//! Self-signed TLS for the personal sync server — the default transport for pairing,
//! learning-sync and the model pull. The server is reached by bare IP (`10.0.2.2` from
//! the emulator, a LAN/tailnet address in the field), so there is no CA to lean on and no
//! hostname to verify. Instead the server mints **one persisted self-signed certificate**
//! and the pairing QR carries the SHA-256 of that cert's DER `SubjectPublicKeyInfo` (the
//! [`ServerTls::fingerprint`]). The phone pins that fingerprint when it scans — trust on
//! first pairing, keyed by the out-of-band QR — and every later sync/model hop verifies the
//! presented cert against it. That defeats a LAN man-in-the-middle against the self-signed
//! cert without a CA, DNS, or public exposure.
//!
//! The cert is **persisted** (`sync-tls-cert.pem` + `sync-tls-key.pem`) so the fingerprint
//! is stable across restarts — regenerating on every boot would silently invalidate every
//! paired device's pin. The private key is the server's TLS identity; on unix it is written
//! `0600`.
//!
//! Desktop-only: like the rest of `idiolect-sync-server`, none of this is compiled into the
//! Android `.so`. We terminate TLS with `tokio-rustls` and serve the same [`axum::Router`]
//! the cleartext path serves, via the exact hyper-util connection loop `axum::serve` runs
//! internally (axum 0.7 cannot take a non-TCP listener, and `axum-server` 0.7 does not build
//! against this workspace's pinned hyper stack).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use idiolect_common::digest::sha256_hex;
use rustls::ServerConfig;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// The persisted cert + key filenames, beside the device-token store.
const CERT_FILE: &str = "sync-tls-cert.pem";
const KEY_FILE: &str = "sync-tls-key.pem";

/// Failures while loading, generating, or serving the self-signed identity.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("generate self-signed certificate: {0}")]
    Generate(#[from] rcgen::Error),
    #[error("tls material i/o at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("parse persisted tls material: {0}")]
    Parse(String),
    #[error("build rustls server config: {0}")]
    Rustls(#[from] rustls::Error),
}

/// A self-signed server identity: the PEM cert + key to serve with, and the hex SHA-256 of
/// the cert's DER `SubjectPublicKeyInfo` — the pin the pairing QR carries and the phone
/// verifies against the cert it is presented.
pub struct ServerTls {
    cert_pem: String,
    key_pem: String,
    fingerprint: String,
}

impl ServerTls {
    /// Load the persisted identity from `dir` (`sync-tls-cert.pem` + `sync-tls-key.pem`),
    /// generating and saving a fresh self-signed one if either file is absent. Persisting is
    /// what keeps the [`fingerprint`](Self::fingerprint) **stable across restarts**.
    pub fn load_or_generate(dir: &Path) -> Result<Self, TlsError> {
        let cert_path = dir.join(CERT_FILE);
        let key_path = dir.join(KEY_FILE);
        if cert_path.exists() && key_path.exists() {
            let cert_pem = read(&cert_path)?;
            let key_pem = read(&key_path)?;
            let fingerprint = fingerprint_from_key_pem(&key_pem)?;
            return Ok(Self {
                cert_pem,
                key_pem,
                fingerprint,
            });
        }
        Self::generate_and_persist(dir, &cert_path, &key_path)
    }

    fn generate_and_persist(
        dir: &Path,
        cert_path: &Path,
        key_path: &Path,
    ) -> Result<Self, TlsError> {
        // The SAN is cosmetic: the phone pins the SPKI and skips hostname verification (the
        // server is IP-addressed), so the name in the cert is never checked.
        let rcgen::CertifiedKey { cert, key_pair } =
            rcgen::generate_simple_self_signed(vec!["idiolect-sync".to_owned()])?;
        let cert_pem = cert.pem();
        let key_pem = key_pair.serialize_pem();
        // Hash the key pair's SPKI; it is byte-identical to the SPKI embedded in the cert
        // the client is presented (asserted in `the_fingerprint_matches_the_certificate_spki`).
        let fingerprint = sha256_hex(&key_pair.public_key_der());

        std::fs::create_dir_all(dir).map_err(|source| TlsError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        write(cert_path, &cert_pem)?;
        write(key_path, &key_pem)?;
        restrict_key_permissions(key_path);

        Ok(Self {
            cert_pem,
            key_pem,
            fingerprint,
        })
    }

    /// The hex SHA-256 of the DER `SubjectPublicKeyInfo` — 64 lowercase hex chars. This is
    /// the `f=` value in the pairing URI, and exactly what `cert.publicKey.encoded` hashes
    /// to on Android.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The certificate chain (PEM) the server presents.
    #[must_use]
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// The private key (PEM). The server's TLS identity — keep it on the user's own machine.
    #[must_use]
    pub fn key_pem(&self) -> &str {
        &self.key_pem
    }

    /// The rustls server config to terminate TLS with.
    pub fn rustls_config(&self) -> Result<ServerConfig, TlsError> {
        ensure_crypto_provider();
        let certs = rustls_pemfile::certs(&mut self.cert_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| TlsError::Parse(format!("certificate chain: {error}")))?;
        let key = rustls_pemfile::private_key(&mut self.key_pem.as_bytes())
            .map_err(|error| TlsError::Parse(format!("private key: {error}")))?
            .ok_or_else(|| TlsError::Parse("no private key in the persisted pem".to_owned()))?;
        Ok(ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?)
    }

    /// A [`TlsAcceptor`] ready to wrap accepted TCP streams.
    pub fn acceptor(&self) -> Result<TlsAcceptor, TlsError> {
        Ok(TlsAcceptor::from(Arc::new(self.rustls_config()?)))
    }
}

/// Recompute the SPKI fingerprint from a persisted key PEM, so a reloaded identity reports
/// the same pin it minted.
fn fingerprint_from_key_pem(key_pem: &str) -> Result<String, TlsError> {
    let key_pair = rcgen::KeyPair::from_pem(key_pem)?;
    Ok(sha256_hex(&key_pair.public_key_der()))
}

fn read(path: &Path) -> Result<String, TlsError> {
    std::fs::read_to_string(path).map_err(|source| TlsError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write(path: &Path, contents: &str) -> Result<(), TlsError> {
    std::fs::write(path, contents).map_err(|source| TlsError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Best-effort `0600` on the private key (unix only); a failure here is not fatal — the key
/// already lives in the user's private data dir.
#[cfg(unix)]
fn restrict_key_permissions(key_path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_key_permissions(_key_path: &Path) {}

/// Serve `app` over TLS on `listener`, terminating with `acceptor`. Mirrors the hyper-util
/// loop `axum::serve` runs internally, but wraps each accepted stream in TLS first. A failed
/// handshake — e.g. a client that refuses the self-signed cert because the presented SPKI
/// does not match its pin — simply drops that connection and the loop continues.
pub async fn serve_tls(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    app: axum::Router,
) -> io::Result<()> {
    loop {
        let (tcp, _remote) = match listener.accept().await {
            Ok(pair) => pair,
            Err(error) if is_connection_error(&error) => continue,
            Err(error) => return Err(error),
        };
        let acceptor = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let Ok(tls) = acceptor.accept(tcp).await else {
                return; // handshake failed (e.g. a non-pinning / wrong-pin client gave up)
            };
            let io = TokioIo::new(tls);
            let service = TowerToHyperService::new(app);
            let _ = Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(io, service)
                .await;
        });
    }
}

fn is_connection_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}

/// Install a process-default rustls crypto provider once (idempotent). rustls 0.23 requires
/// an explicit provider before any `ServerConfig`/`ClientConfig` is built; we use aws-lc-rs,
/// the default the dependency graph compiles.
pub fn ensure_crypto_provider() {
    use std::sync::Once;
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate(dir: &Path) -> ServerTls {
        ServerTls::load_or_generate(dir).expect("generate tls identity")
    }

    #[test]
    fn a_generated_fingerprint_is_sixty_four_lowercase_hex_chars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fingerprint = generate(dir.path()).fingerprint().to_owned();
        assert_eq!(fingerprint.len(), 64, "sha-256 is 32 bytes = 64 hex chars");
        assert!(
            fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "the pin must be lowercase hex so the QR carries it verbatim: {fingerprint}"
        );
    }

    #[test]
    fn the_fingerprint_is_stable_across_reload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = generate(dir.path());
        let minted = first.fingerprint().to_owned();
        let minted_cert = first.cert_pem().to_owned();

        // A "restart": a second load from the same dir must NOT regenerate — same identity,
        // same pin, or every previously paired device would be locked out.
        let reloaded = generate(dir.path());
        assert_eq!(
            reloaded.fingerprint(),
            minted,
            "reloading the persisted identity must report the same pin"
        );
        assert_eq!(
            reloaded.cert_pem(),
            minted_cert,
            "the persisted cert must be reused, not regenerated"
        );
    }

    #[test]
    fn a_fresh_directory_gets_a_different_identity() {
        let one = tempfile::tempdir().expect("tempdir");
        let two = tempfile::tempdir().expect("tempdir");
        assert_ne!(
            generate(one.path()).fingerprint(),
            generate(two.path()).fingerprint(),
            "each server's self-signed identity carries real entropy"
        );
    }

    #[test]
    fn the_fingerprint_matches_the_certificate_spki() {
        // The contract that ties the server's published pin to what a *client* computes from
        // the cert it is presented: the SPKI the phone hashes (`cert.publicKey.encoded`, and
        // the Rust integration-test verifier via x509-parser) is byte-identical to the SPKI
        // the server hashed from its key pair. If these ever diverged, pinning would reject
        // every real device.
        use x509_parser::prelude::*;

        let dir = tempfile::tempdir().expect("tempdir");
        let tls = generate(dir.path());

        let (_, pem) = parse_x509_pem(tls.cert_pem().as_bytes()).expect("parse cert pem");
        let cert = pem.parse_x509().expect("parse x509");
        let spki_der = cert.public_key().raw; // DER of the SubjectPublicKeyInfo
        assert_eq!(
            sha256_hex(spki_der),
            tls.fingerprint(),
            "the QR's pin must equal sha256 of the presented cert's SPKI"
        );
    }

    #[test]
    fn the_persisted_identity_builds_a_rustls_server_config() {
        // Smoke test that the generated cert + key are valid serving material (and that the
        // crypto provider installs), so `serve_tls` has something to bind.
        let dir = tempfile::tempdir().expect("tempdir");
        generate(dir.path())
            .rustls_config()
            .expect("the self-signed material builds a rustls ServerConfig");
    }
}
