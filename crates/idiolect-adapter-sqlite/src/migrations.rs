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
    Migration {
        version: 3,
        name: "v1_storage",
        sql: include_str!("../migrations/0003_v1_storage.sql"),
        expected_sha256_hex: "4ad2369a40ec1849275871a2be3cb8455010c6d7c90d23fd6e049c21092c8e6a",
    },
    Migration {
        version: 4,
        name: "text_history",
        sql: include_str!("../migrations/0004_text_history.sql"),
        expected_sha256_hex: "998c9de2480e8efe79ebeaa4f55187d574a0d2ed1090927b8259c6c4fa74d1bb",
    },
    Migration {
        version: 5,
        name: "tray_settings",
        sql: include_str!("../migrations/0005_tray_settings.sql"),
        expected_sha256_hex: "e46c34bcd7b206aa74c08c4a925a1f9311a2107247690c2f2e57bb6cdefdd975",
    },
    Migration {
        version: 6,
        name: "history_app_materialized",
        sql: include_str!("../migrations/0006_history_app_materialized.sql"),
        expected_sha256_hex: "32f2b5263f6fcf0161e97f96da6da15f2569b3cae4915c6a1c81280a4eb233fe",
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
