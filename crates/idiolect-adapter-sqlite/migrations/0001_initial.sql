CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    checksum TEXT NOT NULL
);

CREATE TABLE event_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    event_version INTEGER NOT NULL,
    event_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    created_by TEXT NOT NULL
);

CREATE UNIQUE INDEX event_log_idempotency_key_unique
    ON event_log(idempotency_key);
CREATE INDEX event_log_aggregate_lookup
    ON event_log(aggregate_type, aggregate_id, id);

CREATE TABLE ime_text_sessions (
    id TEXT PRIMARY KEY,
    raw_stt_text TEXT,
    committed_text TEXT,
    state TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    committed_at TEXT,
    cancelled_at TEXT
);

CREATE TABLE ime_edit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    from_text TEXT NOT NULL,
    to_text TEXT NOT NULL,
    event_index INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY(session_id) REFERENCES ime_text_sessions(id)
);

CREATE UNIQUE INDEX ime_edit_events_session_order_unique
    ON ime_edit_events(session_id, event_index);

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
    FOREIGN KEY(session_id) REFERENCES ime_text_sessions(id)
);

CREATE UNIQUE INDEX training_candidates_idempotency_key_unique
    ON training_candidates(idempotency_key);
CREATE INDEX training_candidates_session_lookup
    ON training_candidates(session_id);

CREATE TABLE adapters (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    manifest_digest TEXT NOT NULL,
    metric_report_digest TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    promoted_at TEXT
);

CREATE INDEX adapters_active_user_lookup
    ON adapters(user_id, active);

CREATE TABLE training_runs (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    manifest_digest TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    finished_at TEXT
);

CREATE INDEX training_runs_user_lookup
    ON training_runs(user_id, started_at);
