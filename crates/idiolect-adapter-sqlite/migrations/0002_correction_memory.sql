CREATE TABLE correction_memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    raw_text TEXT NOT NULL,
    corrected_text TEXT NOT NULL,
    confidence REAL NOT NULL,
    occurrence_count INTEGER NOT NULL,
    first_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE UNIQUE INDEX correction_memory_raw_corrected_unique
    ON correction_memory(raw_text, corrected_text);
CREATE INDEX correction_memory_raw_text_lookup
    ON correction_memory(raw_text);
CREATE INDEX correction_memory_corrected_text_lookup
    ON correction_memory(corrected_text);
