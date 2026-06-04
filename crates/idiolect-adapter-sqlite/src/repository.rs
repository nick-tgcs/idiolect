use std::error::Error;
use std::fmt::{Display, Formatter};

use idiolect_common::ids::ImeSessionId;
use idiolect_ports::storage::MetadataStorePort;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::migrations::migrations;

const SESSION_AGGREGATE_TYPE: &str = "ime_text_session";
const STORAGE_ACTOR: &str = "idiolect-adapter-sqlite";
const SESSION_CREATED: &str = "SessionCreated";
const PREEDIT_CORRECTED: &str = "PreeditCorrected";
const SESSION_COMMITTED: &str = "SessionCommitted";
const SESSION_CANCELLED: &str = "SessionCancelled";
const SESSION_STATE_CREATED: &str = "created";
const SESSION_STATE_COMMITTED: &str = "committed";
const SESSION_STATE_CANCELLED: &str = "cancelled";
const TRAINING_SOURCE_ACCEPTED: &str = "accepted_without_edit";
const CAPTURE_QUALITY_LIVE: &str = "live";
const CANCEL_PAYLOAD: &str = "cancelled";

#[derive(Debug)]
struct StoredEvent {
    event_type: String,
    event_json: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqliteStorageErrorKind {
    Backend,
    MigrationChecksumMismatch,
    IdempotencyConflict,
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

    fn serialization(error: serde_json::Error) -> Self {
        Self {
            kind: SqliteStorageErrorKind::Backend,
            message: format!("session id serialization error: {error}"),
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

    fn idempotency_conflict(
        idempotency_key: &str,
        expected_event_type: &str,
        expected_payload: &str,
        actual_event_type: &str,
        actual_payload: &str,
    ) -> Self {
        Self {
            kind: SqliteStorageErrorKind::IdempotencyConflict,
            message: format!(
                "idempotency conflict for {idempotency_key}: expected {expected_event_type} {expected_payload}, found {actual_event_type} {actual_payload}"
            ),
            source: None,
        }
    }

    fn not_found(entity: &str, id: &str) -> Self {
        Self {
            kind: SqliteStorageErrorKind::Backend,
            message: format!("{entity} not found: {id}"),
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

    pub fn event_count_for_test(&self) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*) FROM event_log",
            [],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    pub fn training_candidate_count_for_test(&self) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*) FROM training_candidates",
            [],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    pub fn session_state_for_test(
        &self,
        session_id: ImeSessionId,
    ) -> Result<String, SqliteStorageError> {
        let session_key = Self::session_key(session_id)?;
        let state = backend_result(
            self.connection
                .query_row(
                    "SELECT state FROM ime_text_sessions WHERE id = ?1",
                    [&session_key],
                    |row| row.get(0),
                )
                .optional(),
        )?;
        state.ok_or_else(|| SqliteStorageError::not_found("ime_text_sessions row", &session_key))
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

    fn session_key(session_id: ImeSessionId) -> Result<String, SqliteStorageError> {
        serde_json::to_string(&session_id).map_err(SqliteStorageError::serialization)
    }

    fn existing_event(
        transaction: &Transaction<'_>,
        idempotency_key: &str,
    ) -> Result<Option<StoredEvent>, SqliteStorageError> {
        let event = backend_result(
            transaction
                .query_row(
                    "SELECT event_type, event_json FROM event_log WHERE idempotency_key = ?1",
                    [idempotency_key],
                    |row| {
                        Ok(StoredEvent {
                            event_type: row.get(0)?,
                            event_json: row.get(1)?,
                        })
                    },
                )
                .optional(),
        )?;
        Ok(event)
    }

    fn is_idempotent_duplicate(
        transaction: &Transaction<'_>,
        idempotency_key: &str,
        expected_event_type: &str,
        expected_payload: &str,
    ) -> Result<bool, SqliteStorageError> {
        if let Some(existing) = Self::existing_event(transaction, idempotency_key)? {
            if existing.event_type == expected_event_type && existing.event_json == expected_payload
            {
                return Ok(true);
            }

            return Err(SqliteStorageError::idempotency_conflict(
                idempotency_key,
                expected_event_type,
                expected_payload,
                &existing.event_type,
                &existing.event_json,
            ));
        }

        Ok(false)
    }

    fn create_event(
        transaction: &Transaction<'_>,
        aggregate_id: &str,
        event_type: &str,
        event_json: &str,
        idempotency_key: &str,
    ) -> Result<(), SqliteStorageError> {
        backend_result(transaction.execute(
            "INSERT INTO event_log(
                aggregate_type,
                aggregate_id,
                event_type,
                event_version,
                event_json,
                idempotency_key,
                created_by
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                SESSION_AGGREGATE_TYPE,
                aggregate_id,
                event_type,
                1_i64,
                event_json,
                idempotency_key,
                STORAGE_ACTOR,
            ],
        ))?;
        Ok(())
    }

    fn session_exists(
        transaction: &Transaction<'_>,
        session_key: &str,
    ) -> Result<bool, SqliteStorageError> {
        let count = backend_result(transaction.query_row(
            "SELECT COUNT(*) FROM ime_text_sessions WHERE id = ?1",
            [session_key],
            |row| row.get::<_, i64>(0),
        ))?;
        Ok(count == 1)
    }

    fn existing_raw_text(
        transaction: &Transaction<'_>,
        session_key: &str,
    ) -> Result<Option<String>, SqliteStorageError> {
        let raw_text = backend_result(
            transaction
                .query_row(
                    "SELECT raw_stt_text FROM ime_text_sessions WHERE id = ?1",
                    [session_key],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional(),
        )?;
        Ok(raw_text.flatten())
    }
}

impl MetadataStorePort for SqliteMetadataStore {
    type Error = SqliteStorageError;

    fn create_session(&mut self, raw_stt_text: Option<&str>) -> Result<ImeSessionId, Self::Error> {
        let session_id = ImeSessionId::new();
        let session_key = Self::session_key(session_id)?;
        let idempotency_key = format!("session-created:{session_key}");
        let event_payload = raw_stt_text.unwrap_or("");
        let transaction = backend_result(self.connection.transaction())?;

        backend_result(transaction.execute(
            "INSERT INTO ime_text_sessions(id, raw_stt_text, state) VALUES (?1, ?2, ?3)",
            params![session_key, raw_stt_text, SESSION_STATE_CREATED],
        ))?;
        Self::create_event(
            &transaction,
            &session_key,
            SESSION_CREATED,
            event_payload,
            &idempotency_key,
        )?;

        backend_result(transaction.commit())?;
        Ok(session_id)
    }

    fn record_preedit_change(
        &mut self,
        session_id: ImeSessionId,
        from_text: &str,
        to_text: &str,
        event_index: u32,
    ) -> Result<(), Self::Error> {
        let session_key = Self::session_key(session_id)?;
        let idempotency_key = format!("preedit:{session_key}:{event_index}");
        let event_payload = format!("{from_text}->{to_text}:{event_index}");
        let transaction = backend_result(self.connection.transaction())?;

        if Self::is_idempotent_duplicate(
            &transaction,
            &idempotency_key,
            PREEDIT_CORRECTED,
            &event_payload,
        )? {
            return Ok(());
        }
        if !Self::session_exists(&transaction, &session_key)? {
            return Err(SqliteStorageError::not_found(
                "ime_text_sessions row",
                &session_key,
            ));
        }

        Self::create_event(
            &transaction,
            &session_key,
            PREEDIT_CORRECTED,
            &event_payload,
            &idempotency_key,
        )?;
        backend_result(transaction.execute(
            "INSERT INTO ime_edit_events(session_id, from_text, to_text, event_index)
             VALUES (?1, ?2, ?3, ?4)",
            params![session_key, from_text, to_text, event_index],
        ))?;

        backend_result(transaction.commit())?;
        Ok(())
    }

    fn commit_session(
        &mut self,
        session_id: ImeSessionId,
        committed_text: &str,
        idempotency_key: &str,
    ) -> Result<(), Self::Error> {
        let session_key = Self::session_key(session_id)?;
        let transaction = backend_result(self.connection.transaction())?;

        if Self::is_idempotent_duplicate(
            &transaction,
            idempotency_key,
            SESSION_COMMITTED,
            committed_text,
        )? {
            return Ok(());
        }

        let raw_text = Self::existing_raw_text(&transaction, &session_key)?
            .ok_or_else(|| SqliteStorageError::not_found("ime_text_sessions row", &session_key))?;

        Self::create_event(
            &transaction,
            &session_key,
            SESSION_COMMITTED,
            committed_text,
            idempotency_key,
        )?;
        backend_result(transaction.execute(
            "INSERT INTO ime_text_sessions(id, raw_stt_text, committed_text, state, committed_at)
             VALUES (?1, NULL, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(id) DO UPDATE SET
                committed_text = excluded.committed_text,
                state = excluded.state,
                committed_at = excluded.committed_at",
            params![session_key, committed_text, SESSION_STATE_COMMITTED],
        ))?;
        backend_result(transaction.execute(
            "INSERT INTO training_candidates(
                session_id,
                raw_text,
                corrected_text,
                source,
                trust_score,
                capture_quality,
                idempotency_key
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                session_key,
                raw_text,
                committed_text,
                TRAINING_SOURCE_ACCEPTED,
                1.0_f64,
                CAPTURE_QUALITY_LIVE,
                idempotency_key,
            ],
        ))?;

        backend_result(transaction.commit())?;
        Ok(())
    }

    fn cancel_session(
        &mut self,
        session_id: ImeSessionId,
        idempotency_key: &str,
    ) -> Result<(), Self::Error> {
        let session_key = Self::session_key(session_id)?;
        let transaction = backend_result(self.connection.transaction())?;

        if Self::is_idempotent_duplicate(
            &transaction,
            idempotency_key,
            SESSION_CANCELLED,
            CANCEL_PAYLOAD,
        )? {
            return Ok(());
        }
        if !Self::session_exists(&transaction, &session_key)? {
            return Err(SqliteStorageError::not_found(
                "ime_text_sessions row",
                &session_key,
            ));
        }

        Self::create_event(
            &transaction,
            &session_key,
            SESSION_CANCELLED,
            CANCEL_PAYLOAD,
            idempotency_key,
        )?;
        backend_result(transaction.execute(
            "UPDATE ime_text_sessions
             SET state = CASE state
                    WHEN ?1 THEN state
                    ELSE ?2
                 END,
                 cancelled_at = CASE state
                    WHEN ?1 THEN cancelled_at
                    ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 END
             WHERE id = ?3",
            params![
                SESSION_STATE_COMMITTED,
                SESSION_STATE_CANCELLED,
                session_key
            ],
        ))?;

        backend_result(transaction.commit())?;
        Ok(())
    }
}

fn backend_result<T>(result: rusqlite::Result<T>) -> Result<T, SqliteStorageError> {
    result.map_err(SqliteStorageError::backend)
}
