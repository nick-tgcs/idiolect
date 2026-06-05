# 008 — AI Post-Processing

**Status:** Future  
**Priority:** Low  
**Effort:** Large  

## Problem

MacWhisper Pro offers "automatic spelling, punctuation and grammar improvement in dictation mode" powered by external AI services (ChatGPT, Claude, etc.). Raw ASR output often has minor errors that a language model can fix — homophones, missing punctuation, awkward phrasing.

## Proposal

Add an optional post-processing step that sends the transcript through an AI model for correction before showing it as preedit.

### Config

```toml
[post_processing]
enabled = false
# "local" or "api"
mode = "local"

# Local mode: uses a small grammar/spelling model
# (future: could use a local LLM via ollama)

# API mode: sends text to an external service
[post_processing.api]
provider = "openai"  # "openai", "anthropic", "ollama", "custom"
model = "gpt-4o-mini"
api_key = ""         # or env var IDIOLECT_AI_API_KEY
endpoint = ""        # custom endpoint

# Prompt template
prompt = "Fix spelling and grammar in this transcript. Preserve the original meaning. Output only the corrected text, no explanation."
```

### Data flow

```
ASR produces "restart trafik"
  → (optional) FillerRemovalUseCase: "restart trafik"
  → (optional) PostProcessingUseCase: "restart Traefik"
  → InputMethodPort::show_preedit(session_id, "restart Traefik")
```

### Privacy considerations

- **Local mode**: Text never leaves the machine. This is consistent with Idiolect's privacy-first design.
- **API mode**: Text is sent to an external service. This must be opt-in with clear disclosure. The config defaults to `enabled = false`.

### Tray menu

```
Settings →
  ├─ Remove fillers: ✓
  ├─ AI correction: ✗
  ├─ Retention: [● 1 day] [○ 7 days] [○ 30 days]
  └─ Max entries: [● 10] [○ 25] [○ 50]
```

### Correction memory integration

This is synergistic with Idiolect's existing correction memory (ADR 0003). When the user corrects "trafik" → "Traefik" in the preedit, that correction is learned. AI post-processing could also learn from corrections:

- If the AI suggests "restart traffic" but the user corrects it to "restart Traefik", the correction is stored
- Future AI prompts could include correction memory context: "The user previously corrected 'trafik' to 'Traefik'"

### Implementation

```rust
// crates/idiolect-application/src/use_cases/post_processing.rs

pub trait PostProcessor {
    type Error;
    fn process(&mut self, text: &str) -> Result<String, Self::Error>;
}

// Local: rule-based spelling/grammar fixes
pub struct LocalPostProcessor { /* ... */ }

// API: calls external AI service
pub struct ApiPostProcessor { /* ... */ }
```

## Why not v1

- Adds significant complexity (async HTTP calls, API key management, error handling for network failures)
- Privacy implications require careful UX (opt-in, clear disclosure)
- The correction memory system already handles the most common errors (proper nouns, domain terms)
- Latency: adding an AI round-trip to every transcription adds 0.5-2 seconds of delay
- Dependency on external services contradicts Idiolect's offline-first design

This feature makes more sense after the core tray/history workflow is solid and correction memory has been trained on real user data.