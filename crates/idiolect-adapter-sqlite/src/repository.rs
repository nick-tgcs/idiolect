use std::error::Error;
use std::fmt::{Display, Formatter};

use rusqlite::{params, Connection, OptionalExtension};

use crate::migrations::migrations;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqliteStorageErrorKind {
    Backend,
    MigrationChecksumMismatch,
}

#[derive(Debug)]
pub struct SqliteStorageError {
    kind: SqliteStorageErrorKind,
    message: String,
    source: Option<Box<dyn Error + 'static>>,
}

impl SqliteStorageError {
    #[must_use]
    pub fn kind(&self) -> SqliteStorageErrorKind {
        self.kind
    }

    fn backend(error: rusqlite::Error) -> Self {
        Self {
            kind: SqliteStorageErrorKind::Backend,
            message: format!("sqlite backend error: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn checksum_mismatch(version: i64, expected: &str, actual: &str) -> Self {
        Self {
            kind: SqliteStorageErrorKind::MigrationChecksumMismatch,
            message: format!(
                "migration {version} checksum mismatch: expected {expected}, found {actual}"
            ),
            source: None,
        }
    }
}

impl Display for SqliteStorageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for SqliteStorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref()
    }
}

pub struct SqliteMetadataStore {
    connection: Connection,
}

impl SqliteMetadataStore {
    pub fn open_in_memory() -> Result<Self, SqliteStorageError> {
        let connection = backend_result(Connection::open_in_memory())?;
        Ok(Self { connection })
    }

    pub fn migrate(&mut self) -> Result<(), SqliteStorageError> {
        for migration in migrations() {
            if let Some(stored_checksum) = self.applied_migration_checksum(migration.version)? {
                if stored_checksum != migration.expected_sha256_hex {
                    return Err(SqliteStorageError::checksum_mismatch(
                        migration.version,
                        migration.expected_sha256_hex,
                        &stored_checksum,
                    ));
                }
                continue;
            }

            let transaction = backend_result(self.connection.transaction())?;
            backend_result(transaction.execute_batch(migration.sql))?;
            backend_result(transaction.execute(
                "INSERT INTO schema_migrations(version, name, checksum) VALUES (?1, ?2, ?3)",
                params![
                    migration.version,
                    migration.name,
                    migration.expected_sha256_hex
                ],
            ))?;
            backend_result(transaction.commit())?;
        }
        Ok(())
    }

    pub fn table_exists_for_test(&self, table: &str) -> Result<bool, SqliteStorageError> {
        let count: i64 = backend_result(self.connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        ))?;
        Ok(count == 1)
    }

    pub fn table_columns_for_test(&self, table: &str) -> Result<Vec<String>, SqliteStorageError> {
        let mut statement = backend_result(
            self.connection
                .prepare(&format!("PRAGMA table_info({table})")),
        )?;
        let rows = backend_result(statement.query_map([], |row| row.get::<_, String>(1)))?;
        let columns = backend_result(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        Ok(columns)
    }

    pub fn applied_migration_versions_for_test(&self) -> Result<Vec<i64>, SqliteStorageError> {
        if !self.table_exists_for_test("schema_migrations")? {
            return Ok(Vec::new());
        }

        let mut statement = backend_result(
            self.connection
                .prepare("SELECT version FROM schema_migrations ORDER BY version"),
        )?;
        let rows = backend_result(statement.query_map([], |row| row.get::<_, i64>(0)))?;
        let versions = backend_result(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        Ok(versions)
    }

    pub fn schema_migration_rows_for_test(
        &self,
    ) -> Result<Vec<(i64, String, String)>, SqliteStorageError> {
        if !self.table_exists_for_test("schema_migrations")? {
            return Ok(Vec::new());
        }

        let mut statement = backend_result(
            self.connection
                .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version"),
        )?;
        let rows = backend_result(statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        }))?;
        let migration_rows = backend_result(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        Ok(migration_rows)
    }

    pub fn force_schema_checksum_for_test(
        &mut self,
        version: i64,
        checksum: &str,
    ) -> Result<(), SqliteStorageError> {
        backend_result(self.connection.execute(
            "UPDATE schema_migrations SET checksum = ?1 WHERE version = ?2",
            params![checksum, version],
        ))?;
        Ok(())
    }

    fn applied_migration_checksum(
        &self,
        version: i64,
    ) -> Result<Option<String>, SqliteStorageError> {
        if !self.table_exists_for_test("schema_migrations")? {
            return Ok(None);
        }

        let checksum = backend_result(
            self.connection
                .query_row(
                    "SELECT checksum FROM schema_migrations WHERE version = ?1",
                    [version],
                    |row| row.get::<_, String>(0),
                )
                .optional(),
        )?;
        Ok(checksum)
    }
}

fn backend_result<T>(result: rusqlite::Result<T>) -> Result<T, SqliteStorageError> {
    result.map_err(SqliteStorageError::backend)
}
