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

-- Trigger to populate history on session commit
CREATE TRIGGER IF NOT EXISTS trg_ime_text_history_on_commit
AFTER UPDATE OF state ON ime_text_sessions
WHEN NEW.state = 'committed' AND OLD.state != 'committed'
BEGIN
    INSERT INTO ime_text_history (session_id, text, state, created_at)
    VALUES (NEW.id, NEW.committed_text, 'committed', NEW.committed_at);
END;

-- Trigger to populate history on session cancel
CREATE TRIGGER IF NOT EXISTS trg_ime_text_history_on_cancel
AFTER UPDATE OF state ON ime_text_sessions
WHEN NEW.state = 'cancelled' AND OLD.state != 'cancelled'
BEGIN
    INSERT INTO ime_text_history (session_id, text, state, created_at)
    VALUES (NEW.id, '', 'cancelled', NEW.cancelled_at);
END;