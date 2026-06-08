PRAGMA defer_foreign_keys = ON;

CREATE TABLE IF NOT EXISTS tray_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Default values matching HistoryConfig defaults
INSERT OR IGNORE INTO tray_settings (key, value) VALUES 
    ('retention_days', '1'),
    ('max_entries', '10');

CREATE INDEX IF NOT EXISTS tray_settings_updated_at_lookup
    ON tray_settings(updated_at DESC);