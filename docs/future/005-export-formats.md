# 005 — Export Formats

**Status:** Future  
**Priority:** Low  
**Effort:** Small  

## Problem

MacWhisper exports transcripts as SRT, VTT, CSV, DOCX, PDF, Markdown, and HTML. Speechnotes exports captions and plain text. Idiolect currently has no export capability — history entries are plain text only.

## Proposal

Add export commands to the CLI that convert history entries into common formats.

### CLI interface

```bash
# Export a single entry
idiolect history show 42 --format text    # plain text (default)
idiolect history show 42 --format json     # structured JSON
idiolect history show 42 --format markdown # Markdown

# Export all history
idiolect history export --format json > history.json
idiolect history export --format markdown > history.md

# Future: export with timestamps (requires session audio data)
idiolect history show 42 --format srt
idiolect history show 42 --format vtt
```

### Implementation

Start with plain text, JSON, and Markdown — these require no external dependencies:

```rust
pub enum ExportFormat {
    Text,   // Just the text content
    Json,   // { id, session_id, text, state, created_at }
    Markdown, // # Session <id>\n\n<text>\n\n_<timestamp>_
}
```

SRT and VTT require timestamp data from the session's audio events. This is possible because `ime_text_sessions` has `audio_object_ref` and `ime_text_history` links to sessions, but it requires re-processing the audio to get word-level timestamps. This is a larger feature.

### Port extension

```rust
// Extension to HistoryPort (future)
fn export(&self, format: ExportFormat, filter: Option<ExportFilter>) -> Result<String, Self::Error>;
```

## Why not v1

- The tray menu provides "Insert" and "Copy" for individual entries — that covers the primary use case
- JSON/Markdown export is useful for power users but not critical for the initial tray menu workflow
- SRT/VTT export requires timestamp data that isn't currently stored per-word
- The history schema already stores all the data needed; export is a presentation layer concern that can be added later without schema changes