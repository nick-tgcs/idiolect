use idiolect_adapter_sqlite::migrations::{migration_by_version, migrations};

#[test]
fn migration_list_has_initial_migration() {
    let versions = migrations()
        .iter()
        .map(|migration| migration.version)
        .collect::<Vec<_>>();
    assert_eq!(versions, [1, 2, 3, 4, 5, 6]);
    assert_eq!(migrations()[0].name, "initial");
    assert_eq!(migrations()[1].name, "correction_memory");
    assert_eq!(migrations()[4].name, "tray_settings");
    assert_eq!(migrations()[5].name, "history_app_materialized");
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

#[test]
fn migration_sql_files_use_lf_line_endings() {
    // Catches CRLF checkout on Windows (missing .gitattributes). If this
    // fails on Windows, ensure `.gitattributes` contains `* text=auto eol=lf`.
    for migration in migrations() {
        assert!(
            !migration.sql.contains('\r'),
            "migration '{}' contains CR bytes — checkout with wrong line endings; \
             check that .gitattributes enforces eol=lf",
            migration.name
        );
    }
}
