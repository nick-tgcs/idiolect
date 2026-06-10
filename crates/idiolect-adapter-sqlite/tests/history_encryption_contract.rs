//! Contract tests for at-rest encryption of history text.

use idiolect_adapter_crypto::{ChaCha20Poly1305Cipher, EncryptionPort};
use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ports::storage::MetadataStorePort;
use tempfile::tempdir;

fn cipher(key: u8) -> Box<dyn EncryptionPort + Send + Sync> {
    Box::new(ChaCha20Poly1305Cipher::new([key; 32]))
}

fn commit_secret(store: &mut SqliteMetadataStore, text: &str) {
    let session = store.create_session(Some(text)).unwrap();
    store.commit_session(session, text, "enc-key-1").unwrap();
}

#[test]
fn history_text_is_encrypted_at_rest_and_round_trips() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");

    // Write with encryption enabled.
    let mut store = SqliteMetadataStore::open_path(&db).unwrap();
    store.migrate().unwrap();
    let mut store = store.with_history_cipher(cipher(7));
    commit_secret(&mut store, "launch code 1234");

    // Same key: the plaintext round-trips.
    assert_eq!(
        store.recent_history(10).unwrap()[0].text,
        "launch code 1234"
    );

    // No cipher: the raw column is what is stored at rest — it must not be the
    // plaintext, proving the value is encrypted on disk.
    let plain = SqliteMetadataStore::open_path(&db).unwrap();
    let stored = plain.recent_history(10).unwrap()[0].text.clone();
    assert_ne!(stored, "launch code 1234");
    assert!(!stored.contains("launch"));
}

#[test]
fn wrong_key_does_not_reveal_plaintext() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");

    let mut store = SqliteMetadataStore::open_path(&db).unwrap();
    store.migrate().unwrap();
    let mut store = store.with_history_cipher(cipher(7));
    commit_secret(&mut store, "launch code 1234");

    // A store opened with the wrong key falls back to the raw ciphertext rather
    // than panicking or revealing the plaintext.
    let wrong = SqliteMetadataStore::open_path(&db)
        .unwrap()
        .with_history_cipher(cipher(9));
    assert_ne!(
        wrong.recent_history(10).unwrap()[0].text,
        "launch code 1234"
    );
}

#[test]
fn plaintext_store_is_unaffected_by_encryption_path() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("test.sqlite");

    // No cipher configured: behaviour is unchanged, text stored as-is.
    let mut store = SqliteMetadataStore::open_path(&db).unwrap();
    store.migrate().unwrap();
    commit_secret(&mut store, "plain text entry");

    assert_eq!(
        store.recent_history(10).unwrap()[0].text,
        "plain text entry"
    );
}
