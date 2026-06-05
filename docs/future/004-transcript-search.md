# 004 — Transcript Search

**Status:** Future  
**Priority:** Low  
**Effort:** Medium  

## Problem

MacWhisper's most praised feature is "Full text and speaker search through all your transcripts." As history grows beyond the 10-entry tray submenu, users need a way to find past transcriptions by content.

## Proposal

Add full-text search over the `ime_text_history` table using SQLite FTS5.

### Schema change

```sql
-- Migration 0004 (future)
CREATE VIRTUAL TABLE ime_text_history_fts USING fts5(
    text,
    content='ime_text_history',
    content_rowid='id'
);

-- Triggers to keep FTS in sync
CREATE TRIGGER ime_text_history_fts_insert AFTER INSERT ON ime_text_history BEGIN
    INSERT INTO ime_text_history_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER ime_text_history_fts_delete AFTER DELETE ON ime_text_history BEGIN
    INSERT INTO ime_text_history_fts(ime_text_history_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
```

### CLI interface

```bash
# Search history
idiolect history search "traefik"
idiolect history search "restart"

# Show matching entries with context
idiolect history search --limit 20 "meeting notes"
```

### Port extension

```rust
// Extension to HistoryPort (future)
fn search(&self, query: &str, limit: u32) -> Result<Vec<HistoryEntry>, Self::Error>;
```

### Future GUI

A search dialog could be launched from the tray menu ("Search History…") or via a global hotkey. This would require a lightweight GUI toolkit (e.g., `slint`, `iced`, or a web-based UI served from the daemon).

## Why not v1

- The tray menu already provides access to the 10 most recent entries
- FTS5 adds schema complexity (triggers, virtual tables)
- CLI search is useful but not critical for the initial tray menu workflow
- A search GUI requires choosing a UI framework, which is a separate decision

The `ime_text_history` table is designed to support FTS5 later — the `text` column is separate from metadata, and the `id` column is a stable rowid.