use sha2::{Digest, Sha256};

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
    pub expected_sha256_hex: &'static str,
}

impl Migration {
    #[must_use]
    pub fn sha256_hex(&self) -> String {
        let digest = Sha256::digest(self.sql.as_bytes());
        let mut hex = String::with_capacity(64);
        for byte in digest {
            hex.push(nibble_to_hex(byte >> 4));
            hex.push(nibble_to_hex(byte & 0x0f));
        }
        hex
    }
}

fn nibble_to_hex(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'a' + (nibble - 10)),
        _ => unreachable!("nibble should be four bits"),
    }
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../migrations/0001_initial.sql"),
        expected_sha256_hex: "5df0243d62760ef60263c07710fb4bb9d7966d5220e0c4a09629bb9adeb12470",
    },
    Migration {
        version: 2,
        name: "correction_memory",
        sql: include_str!("../migrations/0002_correction_memory.sql"),
        expected_sha256_hex: "36d0013c7516f39ba8006237ea7aa3ed683c2a1313a506d63c5660472a01b648",
    },
];

#[must_use]
pub fn migrations() -> &'static [Migration] {
    MIGRATIONS
}

#[must_use]
pub fn migration_by_version(version: i64) -> Option<&'static Migration> {
    migrations()
        .iter()
        .find(|migration| migration.version == version)
}
