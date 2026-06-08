//! At-rest encryption building block for Idiolect.
//!
//! Provides an authenticated-encryption port ([`EncryptionPort`]) backed by
//! ChaCha20-Poly1305, plus key-provisioning adapters ([`EncryptionKeyPort`]).
//!
//! Ciphertext layout (hex-encoded for storage in a TEXT column):
//! `hex(nonce[12] || ciphertext || tag[16])`. A fresh random nonce is generated
//! per encryption, so identical plaintexts produce distinct ciphertexts.

use std::fs;
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use thiserror::Error;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed (wrong key or tampered ciphertext)")]
    Decrypt,
    #[error("ciphertext is malformed")]
    MalformedCiphertext,
    #[error("key file io error: {0}")]
    KeyIo(#[from] io::Error),
    #[error("key file is corrupt: expected {KEY_LEN} bytes, found {0}")]
    KeyLength(usize),
}

/// Authenticated encryption of UTF-8 text for storage at rest.
pub trait EncryptionPort {
    /// Encrypts `plaintext`, returning a hex-encoded `nonce || ciphertext` token.
    ///
    /// # Errors
    /// Returns [`CryptoError::Encrypt`] if the underlying AEAD operation fails.
    fn encrypt(&self, plaintext: &str) -> Result<String, CryptoError>;

    /// Decrypts a hex token produced by [`EncryptionPort::encrypt`].
    ///
    /// # Errors
    /// Returns [`CryptoError::Decrypt`] on an authentication failure (wrong key
    /// or tampering) and [`CryptoError::MalformedCiphertext`] if the token is not
    /// a valid hex `nonce || ciphertext`.
    fn decrypt(&self, token: &str) -> Result<String, CryptoError>;
}

/// ChaCha20-Poly1305 cipher over a fixed 256-bit key.
pub struct ChaCha20Poly1305Cipher {
    cipher: ChaCha20Poly1305,
}

impl ChaCha20Poly1305Cipher {
    #[must_use]
    pub fn new(key: [u8; KEY_LEN]) -> Self {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        Self { cipher }
    }
}

impl EncryptionPort for ChaCha20Poly1305Cipher {
    fn encrypt(&self, plaintext: &str) -> Result<String, CryptoError> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| CryptoError::Encrypt)?;
        let mut token = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        token.extend_from_slice(nonce.as_slice());
        token.extend_from_slice(&ciphertext);
        Ok(to_hex(&token))
    }

    fn decrypt(&self, token: &str) -> Result<String, CryptoError> {
        let bytes = from_hex(token)?;
        if bytes.len() < NONCE_LEN {
            return Err(CryptoError::MalformedCiphertext);
        }
        let (nonce_bytes, ciphertext) = bytes.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| CryptoError::Decrypt)?;
        String::from_utf8(plaintext).map_err(|_| CryptoError::Decrypt)
    }
}

/// Provides the symmetric key used to construct a cipher.
pub trait EncryptionKeyPort {
    /// Returns the 256-bit key, generating and persisting it on first use where
    /// the provider supports persistence.
    ///
    /// # Errors
    /// Returns a [`CryptoError`] if the key cannot be loaded or created.
    fn load_or_create_key(&self) -> Result<[u8; KEY_LEN], CryptoError>;
}

/// In-memory key provider for tests and ephemeral use.
pub struct InMemoryKey {
    key: [u8; KEY_LEN],
}

impl InMemoryKey {
    #[must_use]
    pub fn new(key: [u8; KEY_LEN]) -> Self {
        Self { key }
    }

    /// Generates a fresh random key.
    #[must_use]
    pub fn generate() -> Self {
        Self {
            key: random_key(),
        }
    }
}

impl EncryptionKeyPort for InMemoryKey {
    fn load_or_create_key(&self) -> Result<[u8; KEY_LEN], CryptoError> {
        Ok(self.key)
    }
}

/// File-backed key provider. The key is stored as raw bytes in a file with
/// `0600` permissions under the user's data directory — local-first, with no
/// dependency on a running secret-service / D-Bus session.
pub struct FileKey {
    path: PathBuf,
}

impl FileKey {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl EncryptionKeyPort for FileKey {
    fn load_or_create_key(&self) -> Result<[u8; KEY_LEN], CryptoError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let len = bytes.len();
                let key: [u8; KEY_LEN] =
                    bytes.try_into().map_err(|_| CryptoError::KeyLength(len))?;
                Ok(key)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let key = random_key();
                write_key_file(&self.path, &key)?;
                Ok(key)
            }
            Err(error) => Err(CryptoError::KeyIo(error)),
        }
    }
}

fn write_key_file(path: &Path, key: &[u8; KEY_LEN]) -> Result<(), CryptoError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    let mut file = options.open(path)?;
    io::Write::write_all(&mut file, key)?;
    // Ensure permissions are 0600 even if the file already existed.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn random_key() -> [u8; KEY_LEN] {
    let key = ChaCha20Poly1305::generate_key(&mut OsRng);
    let mut bytes = [0_u8; KEY_LEN];
    bytes.copy_from_slice(key.as_slice());
    bytes
}

fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(nibble_to_hex(byte >> 4));
        hex.push(nibble_to_hex(byte & 0x0f));
    }
    hex
}

fn nibble_to_hex(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        _ => char::from(b'a' + (nibble - 10)),
    }
}

fn from_hex(hex: &str) -> Result<Vec<u8>, CryptoError> {
    if !hex.len().is_multiple_of(2) {
        return Err(CryptoError::MalformedCiphertext);
    }
    let bytes = hex.as_bytes();
    let mut out = Vec::with_capacity(hex.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = hex_value(pair[0]).ok_or(CryptoError::MalformedCiphertext)?;
        let low = hex_value(pair[1]).ok_or(CryptoError::MalformedCiphertext)?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChaCha20Poly1305Cipher, CryptoError, EncryptionKeyPort, EncryptionPort, FileKey,
        InMemoryKey,
    };
    use tempfile::tempdir;

    fn cipher() -> ChaCha20Poly1305Cipher {
        ChaCha20Poly1305Cipher::new([7_u8; 32])
    }

    #[test]
    fn roundtrip_recovers_plaintext() {
        let cipher = cipher();
        let token = cipher.encrypt("restart Traefik").unwrap();
        assert_eq!(cipher.decrypt(&token).unwrap(), "restart Traefik");
    }

    #[test]
    fn roundtrip_handles_unicode_and_empty() {
        let cipher = cipher();
        for text in ["", "héllo 世界 🚀"] {
            let token = cipher.encrypt(text).unwrap();
            assert_eq!(cipher.decrypt(&token).unwrap(), text);
        }
    }

    #[test]
    fn each_encryption_uses_a_fresh_nonce() {
        let cipher = cipher();
        let a = cipher.encrypt("same plaintext").unwrap();
        let b = cipher.encrypt("same plaintext").unwrap();
        assert_ne!(a, b, "nonce reuse would make ciphertexts identical");
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let cipher = cipher();
        let mut token = cipher.encrypt("secret").unwrap();
        // Flip the last hex nibble.
        let last = token.pop().unwrap();
        token.push(if last == 'a' { 'b' } else { 'a' });
        assert!(matches!(cipher.decrypt(&token), Err(CryptoError::Decrypt)));
    }

    #[test]
    fn wrong_key_cannot_decrypt() {
        let token = ChaCha20Poly1305Cipher::new([1_u8; 32])
            .encrypt("secret")
            .unwrap();
        let other = ChaCha20Poly1305Cipher::new([2_u8; 32]);
        assert!(matches!(other.decrypt(&token), Err(CryptoError::Decrypt)));
    }

    #[test]
    fn malformed_token_is_rejected() {
        let cipher = cipher();
        assert!(matches!(
            cipher.decrypt("not-hex!"),
            Err(CryptoError::MalformedCiphertext)
        ));
        assert!(matches!(
            cipher.decrypt("abcd"), // valid hex but shorter than a nonce
            Err(CryptoError::MalformedCiphertext)
        ));
    }

    #[test]
    fn file_key_generates_then_reuses_a_stable_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("keys").join("history.key");
        let provider = FileKey::new(&path);

        let first = provider.load_or_create_key().unwrap();
        let second = provider.load_or_create_key().unwrap();
        assert_eq!(first, second, "key must persist across loads");

        let permissions = std::fs::metadata(&path).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&permissions) & 0o777,
            0o600
        );
    }

    #[test]
    fn in_memory_key_is_returned_verbatim() {
        let provider = InMemoryKey::new([9_u8; 32]);
        assert_eq!(provider.load_or_create_key().unwrap(), [9_u8; 32]);
    }
}
