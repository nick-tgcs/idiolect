PRAGMA defer_foreign_keys = ON;

CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    display_name TEXT,
    active_adapter_id TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT
);

INSERT OR IGNORE INTO users (id, display_name)
VALUES ('default', 'default');

CREATE TABLE IF NOT EXISTS utterances (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    audio_path TEXT NOT NULL,
    audio_codec TEXT NOT NULL,
    audio_container TEXT NOT NULL,
    sample_rate_hz INTEGER NOT NULL,
    training_sample_rate_hz INTEGER,
    channels INTEGER NOT NULL,
    bitrate_bps INTEGER,
    duration_ms INTEGER NOT NULL,
    audio_sha256 TEXT,
    raw_stt_text TEXT,
    stt_model TEXT NOT NULL DEFAULT 'unknown',
    adapter_id TEXT,
    confidence REAL,
    language TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (user_id) REFERENCES users(id)
);

INSERT OR IGNORE INTO utterances(
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
    language,
    created_at
)
SELECT
    'utterance:' || replace(id, '"', ''),
    'default',
    'audio/1970/01/01/default/' || ('utterance:' || replace(id, '"', '')) || '.ogg',
    'opus',
    'ogg',
    16000,
    16000,
    1,
    0,
    raw_stt_text,
    'unknown',
    'en',
    created_at
FROM ime_text_sessions;

CREATE INDEX IF NOT EXISTS utterances_user_lookup
    ON utterances(user_id);

CREATE TABLE IF NOT EXISTS utterance_audio_files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    utterance_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    codec TEXT NOT NULL,
    container TEXT NOT NULL,
    sample_rate_hz INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (utterance_id) REFERENCES utterances(id)
);

INSERT OR IGNORE INTO utterance_audio_files(
    utterance_id,
    file_path,
    codec,
    container,
    sample_rate_hz,
    duration_ms,
    created_at
)
SELECT
    id,
    audio_path,
    audio_codec,
    audio_container,
    sample_rate_hz,
    duration_ms,
    created_at
FROM utterances;

CREATE INDEX IF NOT EXISTS utterance_audio_files_utterance_lookup
    ON utterance_audio_files(utterance_id);

ALTER TABLE ime_text_sessions RENAME TO ime_text_sessions_legacy;

CREATE TABLE ime_text_sessions (
    id TEXT PRIMARY KEY,
    raw_stt_text TEXT,
    committed_text TEXT,
    state TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    committed_at TEXT,
    cancelled_at TEXT,
    utterance_id TEXT NOT NULL,
    user_id TEXT NOT NULL DEFAULT 'default',
    platform TEXT NOT NULL DEFAULT 'unknown',
    input_backend TEXT NOT NULL DEFAULT 'unknown',
    target_app_name TEXT,
    target_app_class TEXT,
    target_window_title TEXT,
    session_state TEXT NOT NULL DEFAULT 'created',
    initial_preedit_text TEXT,
    final_preedit_text TEXT,
    surrounding_text_before TEXT,
    surrounding_text_after TEXT,
    edit_capture_quality TEXT NOT NULL DEFAULT 'live',
    started_at TEXT NOT NULL DEFAULT '1970-01-01T00:00:00.000Z',
    last_observed_at TEXT,
    FOREIGN KEY (utterance_id) REFERENCES utterances(id),
    FOREIGN KEY (user_id) REFERENCES users(id)
);

INSERT INTO ime_text_sessions(
    id,
    raw_stt_text,
    committed_text,
    state,
    created_at,
    committed_at,
    cancelled_at,
    utterance_id,
    user_id,
    platform,
    input_backend,
    session_state,
    initial_preedit_text,
    final_preedit_text,
    surrounding_text_before,
    surrounding_text_after,
    edit_capture_quality,
    started_at,
    last_observed_at
)
SELECT
    id,
    raw_stt_text,
    committed_text,
    state,
    created_at,
    committed_at,
    cancelled_at,
    'utterance:' || replace(id, '"', ''),
    'default',
    'unknown',
    'unknown',
    COALESCE(NULLIF(state, ''), 'created'),
    raw_stt_text,
    COALESCE(committed_text, raw_stt_text),
    '',
    '',
    'live',
    created_at,
    created_at
FROM ime_text_sessions_legacy;

DROP TABLE ime_text_sessions_legacy;

ALTER TABLE ime_edit_events RENAME TO ime_edit_events_legacy;

CREATE TABLE ime_edit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    from_text TEXT,
    to_text TEXT,
    event_index INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    text_session_id TEXT NOT NULL,
    event_type TEXT NOT NULL DEFAULT 'preedit_update',
    cursor_position INTEGER,
    surrounding_text TEXT,
    timestamp_ms INTEGER NOT NULL,
    FOREIGN KEY (session_id) REFERENCES ime_text_sessions(id),
    FOREIGN KEY (text_session_id) REFERENCES ime_text_sessions(id)
);

INSERT INTO ime_edit_events(
    id,
    session_id,
    from_text,
    to_text,
    event_index,
    created_at,
    text_session_id,
    event_type,
    cursor_position,
    surrounding_text,
    timestamp_ms
)
SELECT
    id,
    session_id,
    from_text,
    to_text,
    event_index,
    created_at,
    session_id,
    'preedit_update',
    NULL,
    '',
    CAST((julianday(created_at) - 2440587.5) * 86400000 AS INTEGER)
FROM ime_edit_events_legacy;

DROP TABLE ime_edit_events_legacy;

CREATE UNIQUE INDEX ime_edit_events_session_order_unique
    ON ime_edit_events(session_id, event_index);

ALTER TABLE training_candidates RENAME TO training_candidates_legacy;

CREATE TABLE training_candidates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    raw_text TEXT NOT NULL,
    corrected_text TEXT NOT NULL,
    source TEXT NOT NULL,
    trust_score REAL NOT NULL,
    capture_quality TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    utterance_id TEXT NOT NULL,
    text_session_id TEXT NOT NULL,
    candidate_transcript TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'captured',
    classifier_label TEXT,
    classifier_model TEXT,
    classifier_reason TEXT,
    classified_at TEXT,
    FOREIGN KEY (session_id) REFERENCES ime_text_sessions(id),
    FOREIGN KEY (utterance_id) REFERENCES utterances(id),
    FOREIGN KEY (text_session_id) REFERENCES ime_text_sessions(id)
);

INSERT INTO training_candidates(
    id,
    session_id,
    raw_text,
    corrected_text,
    source,
    trust_score,
    capture_quality,
    idempotency_key,
    created_at,
    utterance_id,
    text_session_id,
    candidate_transcript,
    status
)
SELECT
    tc.id,
    tc.session_id,
    tc.raw_text,
    tc.corrected_text,
    tc.source,
    tc.trust_score,
    tc.capture_quality,
    tc.idempotency_key,
    tc.created_at,
    s.utterance_id,
    tc.session_id,
    COALESCE(NULLIF(tc.corrected_text, ''), tc.raw_text),
    'captured'
FROM training_candidates_legacy AS tc
JOIN ime_text_sessions AS s ON s.id = tc.session_id;

DROP TABLE training_candidates_legacy;

CREATE UNIQUE INDEX training_candidates_idempotency_key_unique
    ON training_candidates(idempotency_key);
CREATE INDEX training_candidates_session_lookup
    ON training_candidates(session_id);
CREATE INDEX training_candidates_utterance_lookup
    ON training_candidates(utterance_id);
CREATE INDEX training_candidates_text_session_lookup
    ON training_candidates(text_session_id);

ALTER TABLE correction_memory RENAME TO correction_memory_legacy;

CREATE TABLE correction_memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL DEFAULT 'default',
    raw_text TEXT NOT NULL,
    corrected_text TEXT NOT NULL,
    confidence REAL NOT NULL,
    occurrence_count INTEGER NOT NULL,
    first_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (user_id) REFERENCES users(id)
);

INSERT INTO correction_memory(
    id,
    user_id,
    raw_text,
    corrected_text,
    confidence,
    occurrence_count,
    first_seen_at,
    last_seen_at
)
SELECT
    id,
    'default',
    raw_text,
    corrected_text,
    confidence,
    occurrence_count,
    first_seen_at,
    last_seen_at
FROM correction_memory_legacy;

DROP TABLE correction_memory_legacy;

CREATE UNIQUE INDEX correction_memory_user_raw_corrected_unique
    ON correction_memory(user_id, raw_text, corrected_text);
CREATE INDEX correction_memory_user_raw_text_lookup
    ON correction_memory(user_id, raw_text);
CREATE INDEX correction_memory_user_corrected_text_lookup
    ON correction_memory(user_id, corrected_text);

ALTER TABLE training_runs RENAME TO training_runs_legacy;

INSERT OR IGNORE INTO users(id, display_name)
SELECT DISTINCT user_id, user_id
FROM training_runs_legacy;

CREATE TABLE training_runs (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    manifest_digest TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    finished_at TEXT,
    base_model TEXT NOT NULL DEFAULT 'unknown',
    previous_adapter_id TEXT,
    new_adapter_id TEXT,
    num_training_examples INTEGER,
    num_validation_examples INTEGER,
    num_holdout_examples INTEGER,
    notes TEXT,
    FOREIGN KEY (user_id) REFERENCES users(id)
);

INSERT INTO training_runs(
    id,
    user_id,
    manifest_digest,
    status,
    started_at,
    finished_at,
    base_model,
    previous_adapter_id,
    new_adapter_id,
    num_training_examples,
    num_validation_examples,
    num_holdout_examples,
    notes
)
SELECT
    id,
    user_id,
    manifest_digest,
    status,
    started_at,
    finished_at,
    'unknown',
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL
FROM training_runs_legacy;

DROP TABLE training_runs_legacy;

CREATE INDEX training_runs_user_lookup
    ON training_runs(user_id, started_at);

ALTER TABLE adapters RENAME TO adapters_legacy;

INSERT OR IGNORE INTO users(id, display_name)
SELECT DISTINCT user_id, user_id
FROM adapters_legacy;

CREATE TABLE adapters (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    manifest_digest TEXT NOT NULL,
    metric_report_digest TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    promoted_at TEXT,
    base_model TEXT NOT NULL DEFAULT 'unknown',
    adapter_type TEXT NOT NULL DEFAULT 'lora',
    adapter_path TEXT NOT NULL DEFAULT '',
    training_run_id TEXT,
    metrics TEXT,
    FOREIGN KEY (user_id) REFERENCES users(id),
    FOREIGN KEY (training_run_id) REFERENCES training_runs(id)
);

INSERT INTO adapters(
    id,
    user_id,
    artifact_digest,
    manifest_digest,
    metric_report_digest,
    active,
    created_at,
    promoted_at,
    base_model,
    adapter_type,
    adapter_path,
    training_run_id,
    metrics
)
SELECT
    id,
    user_id,
    artifact_digest,
    manifest_digest,
    metric_report_digest,
    active,
    created_at,
    promoted_at,
    'unknown',
    'lora',
    '',
    NULL,
    NULL
FROM adapters_legacy;

DROP TABLE adapters_legacy;

CREATE INDEX adapters_active_user_lookup
    ON adapters(user_id, active);

CREATE TABLE IF NOT EXISTS manifests (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    split TEXT NOT NULL,
    manifest_path TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    manifest_digest TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    finalized_at TEXT,
    FOREIGN KEY (user_id) REFERENCES users(id)
);

CREATE INDEX IF NOT EXISTS manifests_user_lookup
    ON manifests(user_id);

CREATE TABLE IF NOT EXISTS manifest_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    manifest_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    training_candidate_id INTEGER NOT NULL,
    split TEXT NOT NULL,
    added_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (manifest_id) REFERENCES manifests(id),
    FOREIGN KEY (user_id) REFERENCES users(id),
    FOREIGN KEY (training_candidate_id) REFERENCES training_candidates(id)
);

CREATE INDEX IF NOT EXISTS manifest_items_manifest_lookup
    ON manifest_items(manifest_id);
CREATE INDEX IF NOT EXISTS manifest_items_training_candidate_lookup
    ON manifest_items(training_candidate_id);

CREATE TABLE IF NOT EXISTS adapter_derivations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    from_adapter_id TEXT,
    to_adapter_id TEXT,
    trigger_reason TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (user_id) REFERENCES users(id),
    FOREIGN KEY (from_adapter_id) REFERENCES adapters(id),
    FOREIGN KEY (to_adapter_id) REFERENCES adapters(id)
);

CREATE INDEX IF NOT EXISTS adapter_derivations_user_lookup
    ON adapter_derivations(user_id);

CREATE TABLE IF NOT EXISTS retention_tombstones (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    reason TEXT NOT NULL,
    details TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (user_id) REFERENCES users(id)
);

CREATE INDEX IF NOT EXISTS retention_tombstones_user_lookup
    ON retention_tombstones(user_id);

CREATE TRIGGER IF NOT EXISTS trg_ime_text_sessions_sync_state
AFTER UPDATE OF state ON ime_text_sessions
WHEN NEW.session_state IS NULL OR NEW.session_state != NEW.state
BEGIN
    UPDATE ime_text_sessions
    SET session_state = NEW.state
    WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_retention_tombstones_on_user_delete
AFTER INSERT ON event_log
WHEN NEW.aggregate_type = 'user' AND NEW.event_type = 'UserDataDeleted'
BEGIN
    INSERT INTO retention_tombstones(user_id, reason, details, created_at)
    VALUES (NEW.aggregate_id, 'privacy_delete', NEW.event_json, NEW.created_at);
END;
