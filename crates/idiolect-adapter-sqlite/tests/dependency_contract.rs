use rusqlite::Connection;
use sha2::{Digest, Sha256};

#[test]
fn sqlite_and_checksum_dependencies_are_available() {
    let connection = Connection::open_in_memory().expect("sqlite should open in memory");
    connection
        .execute_batch("CREATE TABLE smoke(id INTEGER PRIMARY KEY);")
        .expect("sqlite should execute smoke schema");

    let digest = Sha256::digest(b"idiolect-storage-smoke");
    let digest_hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(digest_hex.len(), 64);
}
