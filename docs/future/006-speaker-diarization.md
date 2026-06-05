# 006 — Speaker Diarization

**Status:** Future  
**Priority:** Low  
**Effort:** Large  

## Problem

When multiple people speak (meetings, interviews), transcripts without speaker labels are hard to follow. MacWhisper supports manual speaker labeling and automatic speaker recognition (Pro). Speechnotes offers automatic speaker tagging/diarization. whisper.cpp has `tinydiarize` for basic speaker turn detection.

## Proposal

Add speaker labels to transcripts, either from the ASR model or from a post-processing step.

### Approaches

#### A. Whisper tinydiarize

whisper.cpp supports `--tinydiarize` which inserts `(SPEAKER_TURN)` tokens. This is the simplest approach:

- Enable `--tinydiarize` in the Whisper adapter
- Parse `(SPEAKER_TURN)` tokens from the transcript
- Split text into segments with speaker labels (Speaker 1, Speaker 2, etc.)

Limitations: Only detects speaker turns, not speaker identity. No way to label "Alice" vs "Bob."

#### B. PyAnnote / pyannote-audio

A dedicated speaker diarization pipeline that produces speaker segments with timestamps. More accurate than tinydiarize but requires a separate model and more compute.

- Run as a separate processing step after ASR
- Align diarization segments with transcript words by timestamp
- Requires word-level timestamps from Whisper

#### C. Manual speaker labeling

Let the user assign speaker names to segments after transcription:

```
[Speaker 1]: So let's restart Traefik
[Speaker 2]: Sounds good, I'll check the logs

→ User labels in the tray or CLI:
  [Alice]: So let's restart Traefik
  [Bob]: Sounds good, I'll check the logs
```

### Data model changes

```sql
-- Future migration
ALTER TABLE ime_text_history ADD COLUMN speakers TEXT; -- JSON: [{"name": "Alice", "segments": [[0, 5.2]]}]
```

Or a separate table:

```sql
CREATE TABLE ime_speaker_labels (
    id INTEGER PRIMARY KEY,
    history_id INTEGER NOT NULL REFERENCES ime_text_history(id),
    speaker_name TEXT NOT NULL,
    start_offset_ms INTEGER NOT NULL,
    end_offset_ms INTEGER NOT NULL,
    segment_text TEXT NOT NULL
);
```

### Tray menu

```
Recent History →
  ├─ "restart Traefik" (Speaker 1)
  ├─ "check the logs" (Speaker 2)
  └─ Clear History
```

## Why not v1

- Idiolect's primary use case is single-speaker dictation (talking into your computer), not meeting transcription
- Diarization requires significant additional compute and model downloads
- The `ime_text_history` schema can be extended later without breaking changes
- tinydiarize is the easiest on-ramp but still requires changes to the Whisper adapter and transcript parsing

This feature becomes more relevant when Idiolect adds meeting recording (see future doc 007).