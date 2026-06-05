# 002 — Filler Word Removal

**Status:** Future  
**Priority:** Medium  
**Effort:** Small  

## Problem

Spontaneous speech contains filler words ("um", "uh", "uhh", "like", "you know") that make transcripts less useful for text insertion. MacWhisper Pro offers "Automatically remove ums, uhhs and other similar filler words" and it's a frequently praised feature.

## Proposal

Add a post-processing step in the application layer that strips filler words from the transcript before showing it as preedit. This runs after ASR produces text and before `InputMethodPort::show_preedit`.

### Filler list

Start with a configurable list of English filler words:

```
um, uh, uhh, hmm, mm, mm-hmm, uh-huh, huh, like, you know, I mean
```

The list is stored in config and can be extended by the user.

### Processing

```rust
// crates/idiolect-application/src/use_cases/filler_removal.rs

pub fn remove_fillers(text: &str, filler_words: &[String]) -> String {
    // Word-boundary-aware replacement
    // "um, restart Traefik" → "restart Traefik"
    // "uh huh" → "" (both words are fillers)
    // "hummus" → "hummus" (not a filler match, word boundary)
}
```

Key rules:
- Match on word boundaries only (regex `\b{filler}\b`)
- Collapse multiple spaces left behind
- If the entire result is empty, return the original text (don't commit nothing)
- Case-insensitive matching

### Config

```toml
[filler_removal]
enabled = true
words = ["um", "uh", "uhh", "hmm", "mm", "mm-hmm", "uh-huh", "huh"]
```

### Tray menu

Settings submenu gets a toggle:

```
Settings →
  ├─ Remove fillers: ✓
  ├─ Retention: [● 1 day] [○ 7 days] [○ 30 days]
  └─ Max entries: [● 10] [○ 25] [○ 50]
```

### Data flow

```
ASR produces "um, restart Traefik"
  → FillerRemovalUseCase::remove_fillers("um, restart Traefik", &config.filler_words)
  → "restart Traefik"
  → InputMethodPort::show_preedit(session_id, "restart Traefik")
```

## Crate changes

| Crate | Change |
|---|---|
| `idiolect-common` | Add `FillerRemovalConfig` to `IdiolectConfig` |
| `idiolect-application` | New `filler_removal.rs` use case |
| `idiolectd` | Apply filler removal before `show_preedit` |

## Why not in the ASR model?

Whisper sometimes produces fillers and sometimes doesn't, depending on the model and audio. Post-processing is model-agnostic, configurable, and reversible (the original text is still stored in history with fillers intact).

## Future enhancements

- Per-language filler word lists
- Learn which fillers the user actually uses (from history) and suggest adding them
- "Smart" removal that preserves intentional "um" in certain contexts (unlikely to be worth the complexity)