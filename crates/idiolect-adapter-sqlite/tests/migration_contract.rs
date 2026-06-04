use idiolect_adapter_sqlite::migrations::{migration_by_version, migrations};

#[test]
fn migration_list_has_initial_migration() {
    let versions = migrations()
        .iter()
        .map(|migration| migration.version)
        .collect::<Vec<_>>();
    assert_eq!(versions, [1]);
    assert_eq!(migrations()[0].name, "initial");
}

#[test]
fn migration_checksums_match_embedded_catalog() {
    for migration in migrations() {
        assert_eq!(migration.sha256_hex(), migration.expected_sha256_hex);
    }
}

#[test]
fn migration_by_version_rejects_unknown_version() {
    assert!(migration_by_version(99).is_none());
}
