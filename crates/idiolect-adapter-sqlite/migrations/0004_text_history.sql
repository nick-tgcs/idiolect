PRAGMA defer_foreign_keys = ON;

CREATE TABLE IF NOT EXISTS ime_text_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    text TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('committed', 'cancelled')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY(session_id) REFERENCES ime_text_sessions(id)
);

CREATE INDEX IF NOT EXISTS ime_text_history_created_at_lookup
    ON ime_text_history(created_at DESC);
CREATE INDEX IF NOT EXISTS ime_text_history_session_lookup
    ON ime_text_history(session_id);