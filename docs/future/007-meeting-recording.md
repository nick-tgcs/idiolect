# 007 — Meeting Recording

**Status:** Future  
**Priority:** Low  
**Effort:** Large  

## Problem

MacWhisper can "automatically record meetings in Zoom, Teams, Webex, Skype, Chime, Discord and more" by capturing system audio. This turns Idiolect from a dictation tool into a meeting transcription tool.

## Proposal

Add the ability to capture system audio (not just microphone input) and transcribe it in real-time.

### System audio capture

On Linux, system audio capture uses PulseAudio/PipeWire:

```rust
// Uses the cpal crate (already a dependency)
// Configure cpal to capture from the monitor source
// PipeWire: capture from the default sink monitor
// PulseAudio: capture from the default source monitor
```

### Config

```toml
[audio]
# "microphone" or "system" or "both"
input_source = "microphone"

# When "system" or "both", which sink to monitor
system_sink = "default"
```

### Tray menu

```
Settings →
  ├─ Input Source: [● Microphone] [○ System Audio] [○ Both]
  └─ ...
```

### Challenges

1. **Permission**: Capturing system audio may require PulseAudio/PipeWire permissions
2. **Echo**: If both microphone and system audio are captured, the user's own voice appears twice
3. **Privacy**: System audio recording is more sensitive than microphone recording — the user may not want all system audio stored
4. **Speaker attribution**: Without diarization, there's no way to tell who said what in a meeting recording
5. **Continuous recording**: Meetings can be hours long. The current VAD + segment approach may need adjustment for long sessions

### Data model changes

```sql
-- Future: mark sessions as meeting recordings
ALTER TABLE ime_text_history ADD COLUMN source TEXT NOT NULL DEFAULT 'microphone';
-- Values: 'microphone', 'system', 'both'
```

## Why not v1

- The primary use case is dictation, not meeting recording
- System audio capture has significant privacy implications
- Speaker diarization (doc 006) is a prerequisite for useful meeting transcripts
- Long-session handling requires changes to the VAD and ASR pipeline
- The tray menu and history features are the foundation; meeting recording builds on top of them