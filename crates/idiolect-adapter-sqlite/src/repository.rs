use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::Path;

use idiolect_common::ids::ImeSessionId;
use idiolect_ports::storage::{AudioStorePort, MetadataStorePort};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::audio_store::{FileAudioStore, FileAudioStoreError};
use crate::migrations::migrations;

const SESSION_AGGREGATE_TYPE: &str = "ime_text_session";
const STORAGE_ACTOR: &str = "idiolect-adapter-sqlite";
const SESSION_CREATED: &str = "SessionCreated";
const PREEDIT_CORRECTED: &str = "PreeditCorrected";
const SESSION_COMMITTED: &str = "SessionCommitted";
const SESSION_CANCELLED: &str = "SessionCancelled";
const USER_AGGREGATE_TYPE: &str = "user";
const USER_DATA_DELETED: &str = "UserDataDeleted";
const SESSION_STATE_CREATED: &str = "created";
const SESSION_STATE_COMMITTED: &str = "committed";
const SESSION_STATE_CANCELLED: &str = "cancelled";
const TRAINING_SOURCE_ACCEPTED: &str = "accepted_without_edit";
const TRAINING_STATUS_CAPTURED: &str = "captured";
const CAPTURE_QUALITY_LIVE: &str = "live";
const CANCEL_PAYLOAD: &str = "cancelled";
const DEFAULT_USER_ID: &str = "default";
const DEFAULT_PLATFORM: &str = "linux";
const DEFAULT_INPUT_BACKEND: &str = "fcitx5";
const DEFAULT_AUDIO_CODEC: &str = "opus";
const DEFAULT_AUDIO_CONTAINER: &str = "ogg";
const DEFAULT_SAMPLE_RATE_HZ: i64 = 16_000;
const DEFAULT_CHANNELS: i64 = 1;
const DEFAULT_STT_MODEL: &str = "unknown";
const DEFAULT_LANGUAGE: &str = "en";
const ADAPTER_DERIVATION_PROMOTED: &str = "promoted";
const ADAPTER_DERIVATION_REJECTED_PREFIX: &str = "rejected:";
const ADAPTER_DERIVATION_ROLLED_BACK: &str = "rolled_back";
const ADAPTER_DERIVATION_DELETED_SAMPLE: &str = "deleted_training_sample";

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

    fn audio_delete(error: FileAudioStoreError) -> Self {
        Self {
            kind: SqliteStorageErrorKind::Backend,
            message: format!("audio privacy delete failed: {error}"),
            source: Some(Box::new(error)),
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

pub struct PrivacyExportSummary {
    pub user_id: String,
    pub training_candidates: i64,
    pub correction_memory_entries: i64,
    pub user_data_deleted_events: i64,
}

pub struct SessionUtteranceLink {
    pub utterance_id: String,
    pub user_id: String,
    pub session_state: String,
}

pub struct TrainingCandidateLink {
    pub status: String,
    pub text_session_count: i64,
    pub utterance_count: i64,
}

pub struct PrivateRowCounts {
    pub utterances: i64,
    pub text_sessions: i64,
    pub edit_events: i64,
    pub training_candidates: i64,
    pub manifest_items: i64,
    pub tombstones: i64,
}

#[derive(Debug)]
pub struct ForeignKeyReference {
    pub table: String,
    pub from_column: String,
    pub to_column: String,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ManifestTrainingCandidate {
    pub id: i64,
    pub raw_text: String,
    pub corrected_text: String,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ManifestV2TrainingCandidate {
    pub training_candidate_id: i64,
    pub user_id: String,
    pub utterance_id: String,
    pub text_session_id: String,
    pub audio_object_key: String,
    pub audio_digest: String,
    pub raw_transcript: String,
    pub corrected_transcript: String,
    pub source_label: String,
    pub trust_score_bps: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivacyRetentionMode {
    Minimal,
    Balanced,
    Research,
    StrictPrivate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterRegistryStatus {
    Candidate,
    Active,
    Previous,
    Best,
    Rejected,
    RolledBack,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AdapterRegistrationInput {
    pub user_id: String,
    pub adapter_id: String,
    pub artifact_digest: String,
    pub manifest_digest: String,
    pub metric_report_digest: String,
    pub base_model: String,
    pub adapter_path: String,
    pub metrics: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AdapterRegistration {
    user_id: String,
    adapter_id: String,
    artifact_digest: String,
    manifest_digest: String,
    metric_report_digest: String,
    base_model: String,
    adapter_path: String,
    metrics: String,
    training_candidate_ids: Vec<i64>,
}

impl AdapterRegistration {
    #[must_use]
    pub fn new(input: AdapterRegistrationInput) -> Self {
        Self {
            user_id: input.user_id,
            adapter_id: input.adapter_id,
            artifact_digest: input.artifact_digest,
            manifest_digest: input.manifest_digest,
            metric_report_digest: input.metric_report_digest,
            base_model: input.base_model,
            adapter_path: input.adapter_path,
            metrics: input.metrics,
            training_candidate_ids: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_training_candidate_id(mut self, training_candidate_id: i64) -> Self {
        self.training_candidate_ids.push(training_candidate_id);
        self
    }

    fn metrics_json(&self) -> Result<String, SqliteStorageError> {
        if self.training_candidate_ids.is_empty() {
            return Ok(self.metrics.clone());
        }

        let mut metrics = serde_json::from_str::<serde_json::Value>(&self.metrics)
            .unwrap_or_else(|_| serde_json::json!({ "raw_metrics": self.metrics }));
        if !metrics.is_object() {
            metrics = serde_json::json!({ "raw_metrics": self.metrics });
        }
        if let Some(object) = metrics.as_object_mut() {
            object.insert(
                "training_candidate_ids".to_owned(),
                serde_json::json!(&self.training_candidate_ids),
            );
        }
        serde_json::to_string(&metrics).map_err(SqliteStorageError::serialization)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AdapterRegistryEntry {
    adapter_id: String,
    status: AdapterRegistryStatus,
    derived_from_deleted_sample: bool,
}

impl AdapterRegistryEntry {
    #[must_use]
    pub fn adapter_id(&self) -> &str {
        &self.adapter_id
    }

    #[must_use]
    pub fn status(&self) -> AdapterRegistryStatus {
        self.status
    }

    #[must_use]
    pub fn derived_from_deleted_sample(&self) -> bool {
        self.derived_from_deleted_sample
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AdapterRegistrySnapshot {
    entries: Vec<AdapterRegistryEntry>,
    current_active_adapter_id: Option<String>,
    previous_active_adapter_id: Option<String>,
    best_historical_adapter_id: Option<String>,
    historical_adapter_ids: Vec<String>,
}

impl AdapterRegistrySnapshot {
    #[must_use]
    pub fn current_active_adapter_id(&self) -> Option<&str> {
        self.current_active_adapter_id.as_deref()
    }

    #[must_use]
    pub fn previous_active_adapter_id(&self) -> Option<&str> {
        self.previous_active_adapter_id.as_deref()
    }

    #[must_use]
    pub fn best_historical_adapter_id(&self) -> Option<&str> {
        self.best_historical_adapter_id.as_deref()
    }

    #[must_use]
    pub fn historical_adapter_ids(&self) -> &[String] {
        &self.historical_adapter_ids
    }

    #[must_use]
    pub fn entry(&self, adapter_id: &str) -> Option<&AdapterRegistryEntry> {
        self.entries
            .iter()
            .find(|entry| entry.adapter_id == adapter_id)
    }

    #[must_use]
    pub fn status_for(&self, adapter_id: &str) -> Option<AdapterRegistryStatus> {
        self.entry(adapter_id).map(AdapterRegistryEntry::status)
    }
}

struct AdapterRegistryRow {
    adapter_id: String,
    active: bool,
    promoted: bool,
    metrics: String,
}

pub struct SqliteMetadataStore {
    connection: Connection,
}

impl SqliteMetadataStore {
    pub fn open_in_memory() -> Result<Self, SqliteStorageError> {
        Self::from_connection(backend_result(Connection::open_in_memory())?)
    }

    pub fn open_path<P: AsRef<Path>>(path: P) -> Result<Self, SqliteStorageError> {
        Self::from_connection(backend_result(Connection::open(path))?)
    }

    fn from_connection(connection: Connection) -> Result<Self, SqliteStorageError> {
        backend_result(connection.execute_batch("PRAGMA foreign_keys = ON"))?;
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

    pub fn register_adapter_candidate(
        &mut self,
        registration: AdapterRegistration,
    ) -> Result<(), SqliteStorageError> {
        let metrics = registration.metrics_json()?;
        let transaction = backend_result(self.connection.transaction())?;
        Self::ensure_user(&transaction, &registration.user_id)?;
        backend_result(transaction.execute(
            "INSERT INTO adapters(
                id,
                user_id,
                artifact_digest,
                manifest_digest,
                metric_report_digest,
                active,
                base_model,
                adapter_type,
                adapter_path,
                metrics
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, 'lora', ?7, ?8)",
            params![
                registration.adapter_id,
                registration.user_id,
                registration.artifact_digest,
                registration.manifest_digest,
                registration.metric_report_digest,
                registration.base_model,
                registration.adapter_path,
                metrics,
            ],
        ))?;
        backend_result(transaction.commit())?;
        Ok(())
    }

    pub fn promote_adapter(
        &mut self,
        user_id: &str,
        adapter_id: &str,
    ) -> Result<(), SqliteStorageError> {
        let transaction = backend_result(self.connection.transaction())?;
        let current_active = Self::active_adapter_id_in_transaction(&transaction, user_id)?;
        backend_result(transaction.execute(
            "UPDATE adapters SET active = 0 WHERE user_id = ?1",
            params![user_id],
        ))?;
        let promoted = backend_result(transaction.execute(
            "UPDATE adapters
             SET active = 1,
                 promoted_at = COALESCE(promoted_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             WHERE user_id = ?1 AND id = ?2",
            params![user_id, adapter_id],
        ))?;
        if promoted != 1 {
            return Err(SqliteStorageError::not_found("adapter", adapter_id));
        }

        if let Some(previous_adapter_id) = current_active.filter(|current| current != adapter_id) {
            Self::record_adapter_derivation(
                &transaction,
                user_id,
                Some(&previous_adapter_id),
                Some(adapter_id),
                ADAPTER_DERIVATION_PROMOTED,
            )?;
        }

        backend_result(transaction.commit())?;
        Ok(())
    }

    pub fn reject_adapter(
        &mut self,
        user_id: &str,
        adapter_id: &str,
        reason: &str,
    ) -> Result<(), SqliteStorageError> {
        let transaction = backend_result(self.connection.transaction())?;
        let updated = backend_result(transaction.execute(
            "UPDATE adapters SET active = 0 WHERE user_id = ?1 AND id = ?2",
            params![user_id, adapter_id],
        ))?;
        if updated != 1 {
            return Err(SqliteStorageError::not_found("adapter", adapter_id));
        }
        let trigger_reason = format!("{ADAPTER_DERIVATION_REJECTED_PREFIX}{reason}");
        Self::record_adapter_derivation(
            &transaction,
            user_id,
            None,
            Some(adapter_id),
            &trigger_reason,
        )?;
        backend_result(transaction.commit())?;
        Ok(())
    }

    pub fn rollback_adapter(&mut self, user_id: &str) -> Result<(), SqliteStorageError> {
        let transaction = backend_result(self.connection.transaction())?;
        let current = Self::active_adapter_id_in_transaction(&transaction, user_id)?
            .ok_or_else(|| SqliteStorageError::not_found("active adapter", user_id))?;
        let previous = Self::previous_adapter_id_in_transaction(&transaction, user_id)?
            .ok_or_else(|| SqliteStorageError::not_found("rollback adapter", user_id))?;

        backend_result(transaction.execute(
            "UPDATE adapters SET active = 0 WHERE user_id = ?1 AND id = ?2",
            params![user_id, current],
        ))?;
        backend_result(transaction.execute(
            "UPDATE adapters SET active = 1 WHERE user_id = ?1 AND id = ?2",
            params![user_id, previous],
        ))?;
        Self::record_adapter_derivation(
            &transaction,
            user_id,
            Some(&previous),
            Some(&current),
            ADAPTER_DERIVATION_ROLLED_BACK,
        )?;
        backend_result(transaction.commit())?;
        Ok(())
    }

    pub fn mark_adapters_derived_from_deleted_sample(
        &mut self,
        user_id: &str,
        training_candidate_id: i64,
    ) -> Result<(), SqliteStorageError> {
        let rows = self.adapter_registry_rows(user_id)?;
        let transaction = backend_result(self.connection.transaction())?;
        for row in rows {
            if Self::metrics_include_training_candidate(&row.metrics, training_candidate_id) {
                Self::record_adapter_derivation(
                    &transaction,
                    user_id,
                    None,
                    Some(&row.adapter_id),
                    ADAPTER_DERIVATION_DELETED_SAMPLE,
                )?;
            }
        }
        backend_result(transaction.commit())?;
        Ok(())
    }

    pub fn adapter_registry_snapshot(
        &self,
        user_id: &str,
    ) -> Result<AdapterRegistrySnapshot, SqliteStorageError> {
        let rows = self.adapter_registry_rows(user_id)?;
        let rejected = self.adapter_ids_with_derivation(
            user_id,
            &format!("{ADAPTER_DERIVATION_REJECTED_PREFIX}%"),
        )?;
        let rolled_back =
            self.adapter_ids_with_derivation(user_id, ADAPTER_DERIVATION_ROLLED_BACK)?;
        let derived_from_deleted_sample =
            self.adapter_ids_with_derivation(user_id, ADAPTER_DERIVATION_DELETED_SAMPLE)?;
        let current_active_adapter_id = rows
            .iter()
            .find(|row| row.active)
            .map(|row| row.adapter_id.clone());
        let historical_adapter_ids = rows
            .iter()
            .filter(|row| row.promoted)
            .map(|row| row.adapter_id.clone())
            .collect::<Vec<_>>();
        let previous_active_adapter_id = rows
            .iter()
            .rev()
            .find(|row| {
                row.promoted
                    && !row.active
                    && !rejected.contains(&row.adapter_id)
                    && !rolled_back.contains(&row.adapter_id)
            })
            .map(|row| row.adapter_id.clone());
        let best_historical_adapter_id = rows
            .iter()
            .filter(|row| row.promoted)
            .filter_map(|row| {
                Self::personal_wer_delta(&row.metrics).map(|delta| (row.adapter_id.clone(), delta))
            })
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(adapter_id, _)| adapter_id)
            .or_else(|| current_active_adapter_id.clone());

        let entries = rows
            .into_iter()
            .map(|row| {
                let status = if rejected.contains(&row.adapter_id) {
                    AdapterRegistryStatus::Rejected
                } else if rolled_back.contains(&row.adapter_id) {
                    AdapterRegistryStatus::RolledBack
                } else if current_active_adapter_id.as_deref() == Some(row.adapter_id.as_str()) {
                    AdapterRegistryStatus::Active
                } else if previous_active_adapter_id.as_deref() == Some(row.adapter_id.as_str()) {
                    AdapterRegistryStatus::Previous
                } else if best_historical_adapter_id.as_deref() == Some(row.adapter_id.as_str()) {
                    AdapterRegistryStatus::Best
                } else {
                    AdapterRegistryStatus::Candidate
                };
                AdapterRegistryEntry {
                    derived_from_deleted_sample: derived_from_deleted_sample
                        .contains(&row.adapter_id),
                    adapter_id: row.adapter_id,
                    status,
                }
            })
            .collect::<Vec<_>>();

        Ok(AdapterRegistrySnapshot {
            entries,
            current_active_adapter_id,
            previous_active_adapter_id,
            best_historical_adapter_id,
            historical_adapter_ids,
        })
    }

    pub fn insert_manifest_item_for_test(
        &mut self,
        user_id: &str,
        manifest_id: &str,
        training_candidate_id: i64,
    ) -> Result<(), SqliteStorageError> {
        let transaction = backend_result(self.connection.transaction())?;
        Self::ensure_user(&transaction, user_id)?;
        backend_result(transaction.execute(
            "INSERT OR IGNORE INTO manifests(
                id,
                user_id,
                split,
                manifest_path,
                status,
                manifest_digest
             ) VALUES (?1, ?2, 'train', ?3, 'finalized', ?4)",
            params![
                manifest_id,
                user_id,
                format!("manifests/{manifest_id}.json"),
                format!("digest-{manifest_id}"),
            ],
        ))?;
        backend_result(transaction.execute(
            "INSERT INTO manifest_items(
                manifest_id,
                user_id,
                training_candidate_id,
                split
             ) VALUES (?1, ?2, ?3, 'train')",
            params![manifest_id, user_id, training_candidate_id],
        ))?;
        backend_result(transaction.commit())?;
        Ok(())
    }

    pub fn delete_user_data_with_retention_for_test(
        &mut self,
        user_id: &str,
        audio_store: &FileAudioStore,
        mode: PrivacyRetentionMode,
    ) -> Result<(), SqliteStorageError> {
        match mode {
            PrivacyRetentionMode::Minimal
            | PrivacyRetentionMode::Balanced
            | PrivacyRetentionMode::Research
            | PrivacyRetentionMode::StrictPrivate => audio_store
                .privacy_delete_user(user_id)
                .map_err(SqliteStorageError::audio_delete)?,
        }
        self.delete_user_data(user_id)
    }

    pub fn delete_training_candidate_for_privacy_for_test(
        &mut self,
        user_id: &str,
        training_candidate_id: i64,
        mode: PrivacyRetentionMode,
    ) -> Result<(), SqliteStorageError> {
        if matches!(mode, PrivacyRetentionMode::StrictPrivate) {
            self.mark_adapters_derived_from_deleted_sample(user_id, training_candidate_id)?;
        }

        let transaction = backend_result(self.connection.transaction())?;
        Self::ensure_user(&transaction, user_id)?;
        backend_result(transaction.execute(
            "DELETE FROM manifest_items
             WHERE user_id = ?1 AND training_candidate_id = ?2",
            params![user_id, training_candidate_id],
        ))?;
        let deleted = backend_result(transaction.execute(
            "DELETE FROM training_candidates
             WHERE id = ?1
               AND (
                   text_session_id IN (SELECT id FROM ime_text_sessions WHERE user_id = ?2)
                   OR utterance_id IN (SELECT id FROM utterances WHERE user_id = ?2)
               )",
            params![training_candidate_id, user_id],
        ))?;
        if deleted != 1 {
            return Err(SqliteStorageError::not_found(
                "training candidate",
                &training_candidate_id.to_string(),
            ));
        }
        backend_result(transaction.execute(
            "INSERT INTO retention_tombstones(user_id, reason, details)
             VALUES (?1, 'deleted_training_candidate', ?2)",
            params![
                user_id,
                serde_json::json!({
                    "training_candidate_id": training_candidate_id,
                    "mode": format!("{mode:?}"),
                })
                .to_string(),
            ],
        ))?;
        backend_result(transaction.commit())?;
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

    pub fn foreign_keys_for_test(
        &self,
        table: &str,
    ) -> Result<Vec<ForeignKeyReference>, SqliteStorageError> {
        let mut statement = backend_result(
            self.connection
                .prepare(&format!("PRAGMA foreign_key_list({table})")),
        )?;
        let rows = backend_result(statement.query_map([], |row| {
            Ok(ForeignKeyReference {
                table: row.get(2)?,
                from_column: row.get(3)?,
                to_column: row.get(4)?,
            })
        }))?;
        let foreign_keys = backend_result(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        Ok(foreign_keys)
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
        self.training_candidate_count()
    }

    pub fn insert_dangling_training_candidate_for_test(&self) -> Result<(), SqliteStorageError> {
        backend_result(self.connection.execute(
            "INSERT INTO training_candidates(
                session_id,
                raw_text,
                corrected_text,
                source,
                trust_score,
                capture_quality,
                idempotency_key,
                utterance_id,
                text_session_id,
                candidate_transcript,
                status
             ) VALUES (
                'missing-session',
                'raw',
                'corrected',
                'accepted_without_edit',
                1.0,
                'live',
                'dangling-candidate',
                'missing-utterance',
                'missing-session',
                'corrected',
                'captured'
             )",
            [],
        ))?;
        Ok(())
    }

    pub fn session_utterance_link_for_test(
        &self,
        session_id: ImeSessionId,
    ) -> Result<Option<SessionUtteranceLink>, SqliteStorageError> {
        let session_key = Self::session_key(session_id)?;
        let link = backend_result(
            self.connection
                .query_row(
                    "SELECT COALESCE(utterance_id, ''), user_id, session_state
                     FROM ime_text_sessions
                     WHERE id = ?1",
                    [&session_key],
                    |row| {
                        Ok(SessionUtteranceLink {
                            utterance_id: row.get(0)?,
                            user_id: row.get(1)?,
                            session_state: row.get(2)?,
                        })
                    },
                )
                .optional(),
        )?;
        Ok(link)
    }

    pub fn training_candidate_links_for_test(
        &self,
    ) -> Result<Vec<TrainingCandidateLink>, SqliteStorageError> {
        let mut statement = backend_result(self.connection.prepare(
            "SELECT tc.status,
                    COUNT(DISTINCT s.id),
                    COUNT(DISTINCT u.id)
             FROM training_candidates AS tc
             LEFT JOIN ime_text_sessions AS s ON s.id = tc.text_session_id
             LEFT JOIN utterances AS u ON u.id = tc.utterance_id
             GROUP BY tc.id, tc.status
             ORDER BY tc.id",
        ))?;
        let rows = backend_result(statement.query_map([], |row| {
            Ok(TrainingCandidateLink {
                status: row.get(0)?,
                text_session_count: row.get(1)?,
                utterance_count: row.get(2)?,
            })
        }))?;
        let links = backend_result(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        Ok(links)
    }

    pub fn private_row_counts_for_test(
        &self,
        user_id: &str,
    ) -> Result<PrivateRowCounts, SqliteStorageError> {
        Ok(PrivateRowCounts {
            utterances: self.user_utterance_count(user_id)?,
            text_sessions: self.user_text_session_count(user_id)?,
            edit_events: self.user_edit_event_count(user_id)?,
            training_candidates: self.user_training_candidate_count(user_id)?,
            manifest_items: self.user_manifest_item_count(user_id)?,
            tombstones: self.user_tombstone_count(user_id)?,
        })
    }

    pub fn privacy_export_summary(
        &self,
        user_id: &str,
    ) -> Result<PrivacyExportSummary, SqliteStorageError> {
        Ok(PrivacyExportSummary {
            user_id: user_id.to_owned(),
            training_candidates: self.user_training_candidate_count(user_id)?,
            correction_memory_entries: self.user_correction_memory_count(user_id)?,
            user_data_deleted_events: self.user_data_deleted_event_count(user_id)?,
        })
    }

    pub fn training_candidates_for_manifest(
        &self,
        user_id: &str,
    ) -> Result<Vec<ManifestTrainingCandidate>, SqliteStorageError> {
        if self.user_data_deleted_event_count(user_id)? > 0 {
            return Ok(Vec::new());
        }

        let mut statement = backend_result(self.connection.prepare(
            "SELECT tc.id, tc.raw_text, tc.corrected_text
             FROM training_candidates AS tc
             JOIN ime_text_sessions AS s ON s.id = tc.text_session_id
             WHERE s.user_id = ?1
             ORDER BY tc.id",
        ))?;
        let rows = backend_result(statement.query_map([user_id], |row| {
            Ok(ManifestTrainingCandidate {
                id: row.get(0)?,
                raw_text: row.get(1)?,
                corrected_text: row.get(2)?,
            })
        }))?;
        let candidates = backend_result(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        Ok(candidates)
    }

    pub fn training_candidates_for_manifest_v2(
        &self,
        user_id: &str,
    ) -> Result<Vec<ManifestV2TrainingCandidate>, SqliteStorageError> {
        if self.user_data_deleted_event_count(user_id)? > 0 {
            return Ok(Vec::new());
        }

        let mut statement = backend_result(self.connection.prepare(
            "SELECT tc.id,
                    s.user_id,
                    u.id,
                    s.id,
                    u.audio_path,
                    COALESCE(u.audio_sha256, ''),
                    COALESCE(u.raw_stt_text, tc.raw_text),
                    tc.candidate_transcript,
                    tc.source,
                    tc.trust_score
             FROM training_candidates AS tc
             JOIN ime_text_sessions AS s ON s.id = tc.text_session_id
             JOIN utterances AS u ON u.id = tc.utterance_id
             WHERE s.user_id = ?1
             ORDER BY tc.id",
        ))?;
        let rows = backend_result(statement.query_map([user_id], |row| {
            let trust_score = row.get::<_, f64>(9)?;
            Ok(ManifestV2TrainingCandidate {
                training_candidate_id: row.get(0)?,
                user_id: row.get(1)?,
                utterance_id: row.get(2)?,
                text_session_id: row.get(3)?,
                audio_object_key: row.get(4)?,
                audio_digest: row.get(5)?,
                raw_transcript: row.get(6)?,
                corrected_transcript: row.get(7)?,
                source_label: row.get(8)?,
                trust_score_bps: trust_score_bps(trust_score),
            })
        }))?;
        let candidates = backend_result(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        Ok(candidates)
    }

    pub fn set_audio_digest_for_test(
        &self,
        utterance_id: &str,
        audio_digest: &str,
    ) -> Result<(), SqliteStorageError> {
        let updated = backend_result(self.connection.execute(
            "UPDATE utterances SET audio_sha256 = ?1 WHERE id = ?2",
            params![audio_digest, utterance_id],
        ))?;
        if updated == 0 {
            return Err(SqliteStorageError::not_found("utterance", utterance_id));
        }
        Ok(())
    }

    pub fn delete_user_data(&mut self, user_id: &str) -> Result<(), SqliteStorageError> {
        let transaction = backend_result(self.connection.transaction())?;
        let deletion_count: i64 = backend_result(transaction.query_row(
            "SELECT COUNT(*) FROM event_log
             WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND event_type = ?3",
            params![USER_AGGREGATE_TYPE, user_id, USER_DATA_DELETED],
            |row| row.get(0),
        ))?;

        backend_result(transaction.execute(
            "DELETE FROM manifest_items
             WHERE user_id = ?1
                OR manifest_id IN (SELECT id FROM manifests WHERE user_id = ?1)",
            params![user_id],
        ))?;
        backend_result(
            transaction.execute("DELETE FROM manifests WHERE user_id = ?1", params![user_id]),
        )?;
        backend_result(transaction.execute(
            "DELETE FROM training_candidates
             WHERE text_session_id IN (SELECT id FROM ime_text_sessions WHERE user_id = ?1)
                OR utterance_id IN (SELECT id FROM utterances WHERE user_id = ?1)",
            params![user_id],
        ))?;
        backend_result(transaction.execute(
            "DELETE FROM ime_edit_events
             WHERE text_session_id IN (SELECT id FROM ime_text_sessions WHERE user_id = ?1)
                OR session_id IN (SELECT id FROM ime_text_sessions WHERE user_id = ?1)",
            params![user_id],
        ))?;
        backend_result(transaction.execute(
            "DELETE FROM event_log
             WHERE aggregate_type = ?1
               AND aggregate_id IN (SELECT id FROM ime_text_sessions WHERE user_id = ?2)",
            params![SESSION_AGGREGATE_TYPE, user_id],
        ))?;
        backend_result(transaction.execute(
            "DELETE FROM ime_text_sessions WHERE user_id = ?1",
            params![user_id],
        ))?;
        backend_result(transaction.execute(
            "DELETE FROM utterance_audio_files
             WHERE utterance_id IN (SELECT id FROM utterances WHERE user_id = ?1)",
            params![user_id],
        ))?;
        backend_result(transaction.execute(
            "DELETE FROM utterances WHERE user_id = ?1",
            params![user_id],
        ))?;
        backend_result(transaction.execute(
            "DELETE FROM correction_memory WHERE user_id = ?1",
            params![user_id],
        ))?;
        backend_result(transaction.execute(
            "DELETE FROM adapter_derivations WHERE user_id = ?1",
            params![user_id],
        ))?;
        backend_result(
            transaction.execute("DELETE FROM adapters WHERE user_id = ?1", params![user_id]),
        )?;
        backend_result(transaction.execute(
            "DELETE FROM training_runs WHERE user_id = ?1",
            params![user_id],
        ))?;

        let event_payload = serde_json::json!({ "user": user_id }).to_string();
        let idempotency_key = format!("user-data-deleted:{user_id}:{}", deletion_count + 1);
        Self::create_event(
            &transaction,
            USER_AGGREGATE_TYPE,
            user_id,
            USER_DATA_DELETED,
            &event_payload,
            &idempotency_key,
        )?;

        backend_result(transaction.commit())?;
        Ok(())
    }

    pub fn delete_user_data_for_test(&mut self, user_id: &str) -> Result<(), SqliteStorageError> {
        self.delete_user_data(user_id)
    }

    pub fn user_data_deleted_event_count_for_test(
        &self,
        user_id: &str,
    ) -> Result<i64, SqliteStorageError> {
        self.user_data_deleted_event_count(user_id)
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

    fn training_candidate_count(&self) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*) FROM training_candidates",
            [],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    fn user_utterance_count(&self, user_id: &str) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*) FROM utterances WHERE user_id = ?1",
            [user_id],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    fn user_text_session_count(&self, user_id: &str) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*) FROM ime_text_sessions WHERE user_id = ?1",
            [user_id],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    fn user_edit_event_count(&self, user_id: &str) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*)
             FROM ime_edit_events AS e
             JOIN ime_text_sessions AS s ON s.id = e.text_session_id OR s.id = e.session_id
             WHERE s.user_id = ?1",
            [user_id],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    fn user_training_candidate_count(&self, user_id: &str) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*)
             FROM training_candidates AS tc
             LEFT JOIN ime_text_sessions AS s ON s.id = tc.text_session_id
             LEFT JOIN utterances AS u ON u.id = tc.utterance_id
             WHERE s.user_id = ?1 OR u.user_id = ?1",
            [user_id],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    fn user_manifest_item_count(&self, user_id: &str) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*) FROM manifest_items WHERE user_id = ?1",
            [user_id],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    fn user_tombstone_count(&self, user_id: &str) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*) FROM retention_tombstones WHERE user_id = ?1",
            [user_id],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    fn user_correction_memory_count(&self, user_id: &str) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*) FROM correction_memory WHERE user_id = ?1",
            [user_id],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    fn user_data_deleted_event_count(&self, user_id: &str) -> Result<i64, SqliteStorageError> {
        let count = backend_result(self.connection.query_row(
            "SELECT COUNT(*) FROM event_log
             WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND event_type = ?3",
            params![USER_AGGREGATE_TYPE, user_id, USER_DATA_DELETED],
            |row| row.get(0),
        ))?;
        Ok(count)
    }

    fn adapter_registry_rows(
        &self,
        user_id: &str,
    ) -> Result<Vec<AdapterRegistryRow>, SqliteStorageError> {
        let mut statement = backend_result(self.connection.prepare(
            "SELECT id, active, promoted_at IS NOT NULL, COALESCE(metrics, '')
             FROM adapters
             WHERE user_id = ?1
             ORDER BY created_at, id",
        ))?;
        let rows = backend_result(statement.query_map(params![user_id], |row| {
            let active: i64 = row.get(1)?;
            let promoted: i64 = row.get(2)?;
            Ok(AdapterRegistryRow {
                adapter_id: row.get(0)?,
                active: active != 0,
                promoted: promoted != 0,
                metrics: row.get(3)?,
            })
        }))?;
        let rows = backend_result(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        Ok(rows)
    }

    fn adapter_ids_with_derivation(
        &self,
        user_id: &str,
        trigger_pattern: &str,
    ) -> Result<Vec<String>, SqliteStorageError> {
        let mut statement = backend_result(self.connection.prepare(
            "SELECT DISTINCT COALESCE(to_adapter_id, '')
             FROM adapter_derivations
             WHERE user_id = ?1 AND trigger_reason LIKE ?2 AND to_adapter_id IS NOT NULL
             ORDER BY created_at, id",
        ))?;
        let rows = backend_result(
            statement.query_map(params![user_id, trigger_pattern], |row| {
                row.get::<_, String>(0)
            }),
        )?;
        let adapter_ids = backend_result(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        Ok(adapter_ids)
    }

    fn active_adapter_id_in_transaction(
        transaction: &Transaction<'_>,
        user_id: &str,
    ) -> Result<Option<String>, SqliteStorageError> {
        let adapter_id = backend_result(
            transaction
                .query_row(
                    "SELECT id
                     FROM adapters
                     WHERE user_id = ?1 AND active = 1
                     ORDER BY promoted_at DESC, id DESC
                     LIMIT 1",
                    params![user_id],
                    |row| row.get::<_, String>(0),
                )
                .optional(),
        )?;
        Ok(adapter_id)
    }

    fn previous_adapter_id_in_transaction(
        transaction: &Transaction<'_>,
        user_id: &str,
    ) -> Result<Option<String>, SqliteStorageError> {
        let adapter_id = backend_result(
            transaction
                .query_row(
                    "SELECT a.id
                     FROM adapters AS a
                     WHERE a.user_id = ?1
                       AND a.active = 0
                       AND a.promoted_at IS NOT NULL
                       AND NOT EXISTS (
                           SELECT 1
                           FROM adapter_derivations AS d
                           WHERE d.user_id = a.user_id
                             AND d.to_adapter_id = a.id
                             AND (d.trigger_reason LIKE ?2 OR d.trigger_reason = ?3)
                       )
                     ORDER BY a.promoted_at DESC, a.id DESC
                     LIMIT 1",
                    params![
                        user_id,
                        format!("{ADAPTER_DERIVATION_REJECTED_PREFIX}%"),
                        ADAPTER_DERIVATION_ROLLED_BACK,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional(),
        )?;
        Ok(adapter_id)
    }

    fn record_adapter_derivation(
        transaction: &Transaction<'_>,
        user_id: &str,
        from_adapter_id: Option<&str>,
        to_adapter_id: Option<&str>,
        trigger_reason: &str,
    ) -> Result<(), SqliteStorageError> {
        backend_result(transaction.execute(
            "INSERT INTO adapter_derivations(
                user_id,
                from_adapter_id,
                to_adapter_id,
                trigger_reason
             ) VALUES (?1, ?2, ?3, ?4)",
            params![user_id, from_adapter_id, to_adapter_id, trigger_reason],
        ))?;
        Ok(())
    }

    fn personal_wer_delta(metrics: &str) -> Option<f64> {
        let value = serde_json::from_str::<serde_json::Value>(metrics).ok()?;
        value
            .get("wer_personal_delta")
            .or_else(|| value.get("personal_wer_delta"))
            .or_else(|| value.get("wer_personal"))
            .and_then(serde_json::Value::as_f64)
    }

    fn metrics_include_training_candidate(metrics: &str, training_candidate_id: i64) -> bool {
        let Some(value) = serde_json::from_str::<serde_json::Value>(metrics).ok() else {
            return false;
        };
        let Some(ids) = value
            .get("training_candidate_ids")
            .and_then(serde_json::Value::as_array)
        else {
            return false;
        };
        ids.iter()
            .filter_map(serde_json::Value::as_i64)
            .any(|id| id == training_candidate_id)
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
        aggregate_type: &str,
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
                aggregate_type,
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

    fn existing_utterance_id(
        transaction: &Transaction<'_>,
        session_key: &str,
    ) -> Result<String, SqliteStorageError> {
        let utterance_id = backend_result(
            transaction
                .query_row(
                    "SELECT utterance_id FROM ime_text_sessions WHERE id = ?1",
                    [session_key],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional(),
        )?
        .flatten()
        .ok_or_else(|| SqliteStorageError::not_found("ime_text_sessions row", session_key))?;
        Ok(utterance_id)
    }

    fn ensure_default_user(transaction: &Transaction<'_>) -> Result<(), SqliteStorageError> {
        Self::ensure_user(transaction, DEFAULT_USER_ID)
    }

    fn ensure_user(transaction: &Transaction<'_>, user_id: &str) -> Result<(), SqliteStorageError> {
        backend_result(transaction.execute(
            "INSERT OR IGNORE INTO users(id, display_name) VALUES (?1, ?1)",
            params![user_id],
        ))?;
        Ok(())
    }

    fn utterance_key(session_key: &str) -> String {
        format!("utterance:{}", session_key.trim_matches('"'))
    }

    fn audio_path(utterance_key: &str) -> String {
        format!("audio/1970/01/01/{DEFAULT_USER_ID}/{utterance_key}.ogg")
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

        let utterance_id = Self::utterance_key(&session_key);
        let audio_path = Self::audio_path(&utterance_id);
        Self::ensure_default_user(&transaction)?;
        backend_result(transaction.execute(
            "INSERT INTO utterances(
                id,
                user_id,
                audio_path,
                audio_codec,
                audio_container,
                sample_rate_hz,
                training_sample_rate_hz,
                channels,
                duration_ms,
                raw_stt_text,
                stt_model,
                language
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                utterance_id,
                DEFAULT_USER_ID,
                audio_path,
                DEFAULT_AUDIO_CODEC,
                DEFAULT_AUDIO_CONTAINER,
                DEFAULT_SAMPLE_RATE_HZ,
                DEFAULT_CHANNELS,
                0_i64,
                raw_stt_text,
                DEFAULT_STT_MODEL,
                DEFAULT_LANGUAGE,
            ],
        ))?;
        backend_result(transaction.execute(
            "INSERT INTO utterance_audio_files(
                utterance_id,
                file_path,
                codec,
                container,
                sample_rate_hz,
                duration_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                utterance_id,
                audio_path,
                DEFAULT_AUDIO_CODEC,
                DEFAULT_AUDIO_CONTAINER,
                DEFAULT_SAMPLE_RATE_HZ,
                0_i64,
            ],
        ))?;
        backend_result(transaction.execute(
            "INSERT INTO ime_text_sessions(
                id,
                raw_stt_text,
                state,
                utterance_id,
                user_id,
                platform,
                input_backend,
                session_state,
                initial_preedit_text,
                final_preedit_text,
                edit_capture_quality,
                started_at,
                last_observed_at
             ) VALUES (
                ?1,
                ?2,
                ?3,
                ?4,
                ?5,
                ?6,
                ?7,
                ?3,
                ?2,
                ?2,
                ?8,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             )",
            params![
                session_key,
                raw_stt_text,
                SESSION_STATE_CREATED,
                utterance_id,
                DEFAULT_USER_ID,
                DEFAULT_PLATFORM,
                DEFAULT_INPUT_BACKEND,
                CAPTURE_QUALITY_LIVE,
            ],
        ))?;
        Self::create_event(
            &transaction,
            SESSION_AGGREGATE_TYPE,
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
            SESSION_AGGREGATE_TYPE,
            &session_key,
            PREEDIT_CORRECTED,
            &event_payload,
            &idempotency_key,
        )?;
        backend_result(transaction.execute(
            "INSERT INTO ime_edit_events(
                session_id,
                text_session_id,
                from_text,
                to_text,
                event_index,
                event_type,
                timestamp_ms
             ) VALUES (
                ?1,
                ?1,
                ?2,
                ?3,
                ?4,
                'preedit_update',
                CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
             )",
            params![session_key, from_text, to_text, event_index],
        ))?;
        backend_result(transaction.execute(
            "UPDATE ime_text_sessions
             SET final_preedit_text = ?1,
                 last_observed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?2",
            params![to_text, session_key],
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
        let utterance_id = Self::existing_utterance_id(&transaction, &session_key)?;

        Self::create_event(
            &transaction,
            SESSION_AGGREGATE_TYPE,
            &session_key,
            SESSION_COMMITTED,
            committed_text,
            idempotency_key,
        )?;
        backend_result(transaction.execute(
            "UPDATE ime_text_sessions
             SET committed_text = ?1,
                 state = ?2,
                 session_state = ?2,
                 committed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 final_preedit_text = ?1,
                 last_observed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?3",
            params![committed_text, SESSION_STATE_COMMITTED, session_key],
        ))?;
        backend_result(transaction.execute(
            "UPDATE utterances
             SET raw_stt_text = COALESCE(raw_stt_text, ?1)
             WHERE id = ?2",
            params![raw_text, utterance_id],
        ))?;
        backend_result(transaction.execute(
            "INSERT INTO training_candidates(
                session_id,
                raw_text,
                corrected_text,
                source,
                trust_score,
                capture_quality,
                idempotency_key,
                utterance_id,
                text_session_id,
                candidate_transcript,
                status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?1, ?3, ?9)",
            params![
                session_key,
                raw_text,
                committed_text,
                TRAINING_SOURCE_ACCEPTED,
                1.0_f64,
                CAPTURE_QUALITY_LIVE,
                idempotency_key,
                utterance_id,
                TRAINING_STATUS_CAPTURED,
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
            SESSION_AGGREGATE_TYPE,
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
                 session_state = CASE state
                    WHEN ?1 THEN session_state
                    ELSE ?2
                 END,
                 cancelled_at = CASE state
                    WHEN ?1 THEN cancelled_at
                    ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 END,
                 last_observed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
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

fn trust_score_bps(score: f64) -> u16 {
    let clamped = score.clamp(0.0, 1.0);
    (clamped * 10_000.0).round() as u16
}

fn backend_result<T>(result: rusqlite::Result<T>) -> Result<T, SqliteStorageError> {
    result.map_err(SqliteStorageError::backend)
}
