# Idiolect 04 Fixture Audio ASR Codec Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add deterministic fixture audio, fixture ASR, and fixture codec adapters behind the existing port contracts, with integration tests that prove the adapter path before real dependencies are introduced.

**Architecture:** Fixture adapters are pure Rust and deterministic. They exercise `AudioInputPort`, `AsrPort`, and `AudioCodecPort` without CPAL, Whisper, Opus, Silero, Python, hardware, model files, or native libraries.

**Tech Stack:** Rust, workspace port crates, fixture support crate, strict Cargo lint gates, no new third-party runtime dependency.

---

## Scope Boundary

Allowed behavior:

```text
AudioSegment and EncodedAudio concrete DTO fields if child 00 left them incomplete
sine audio fixture
fixture audio adapter
fixture ASR adapter
fixture codec adapter
cross-adapter fixture integration test
```

Forbidden behavior:

```text
CPAL
Opus
Whisper
Silero
ONNX Runtime
PyTorch
PEFT
Python required-path code
hardware capture
model-file download
```

Required gates after every code task:

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

## Task 1: Concrete Audio And Transcript DTOs

**Owner:** Spark worker allowed, gatekeeper reviews port surface  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-ports/src/audio.rs`
- Modify: `crates/idiolect-ports/src/asr.rs`
- Modify: `crates/idiolect-ports/src/codec.rs`
- Modify: `crates/idiolect-ports/src/vad.rs`

- [ ] **Step 1: Write failing DTO tests**

Add tests asserting these exact field names and helper behavior:

```rust
let segment = AudioSegment {
    sample_rate_hz: 16_000,
    channels: 1,
    duration_ms: 1_000,
    samples_f32_mono: vec![0.0; 16_000],
};
assert_eq!(segment.sample_count(), 16_000);

let encoded = EncodedAudio {
    codec_name: "fixture-codec".to_owned(),
    sample_rate_hz: 16_000,
    channels: 1,
    payload: vec![1, 2, 3],
};
assert_eq!(encoded.payload, [1, 2, 3]);

let draft = TranscriptDraft {
    text: "restart traffic".to_owned(),
    metadata: TranscriptMetadata {
        engine_name: "fixture-asr".to_owned(),
        engine_version: "0.1.0".to_owned(),
        confidence: Some(1.0),
    },
};
assert_eq!(draft.metadata.engine_name, "fixture-asr");
```

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-ports --lib
```

Expected: FAIL if DTO fields or helper methods are absent.

- [ ] **Step 3: Implement DTOs**

Define `AudioSegment`, `EncodedAudio`, `TranscriptDraft`, and `TranscriptMetadata` in `idiolect-ports`; re-export them across port modules from the owning port module so all port traits share one DTO type and do not depend on core adapter DTOs. Use these public fields:

```rust
pub struct AudioSegment {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub duration_ms: u32,
    pub samples_f32_mono: Vec<f32>,
}

pub struct EncodedAudio {
    pub codec_name: String,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub payload: Vec<u8>,
}

pub struct TranscriptDraft {
    pub text: String,
    pub metadata: TranscriptMetadata,
}

pub struct TranscriptMetadata {
    pub engine_name: String,
    pub engine_version: String,
    pub confidence: Option<f32>,
}
```

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-ports --lib
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolect-ports
git commit -m "feat: define audio transcript and codec dto fields"
```

## Task 2: Deterministic Sine Fixture

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-test-support/src/lib.rs`
- Create: `crates/idiolect-test-support/src/fixtures.rs`
- Modify: `crates/idiolect-test-support/Cargo.toml`

- [ ] **Step 1: Write failing fixture tests**

Create tests `sine_fixture_has_expected_shape` and `sine_fixture_is_deterministic`. They must assert sample rate `16000`, one channel, duration `1000`, length `16000`, first sample `0.0`, and identical sample vectors across two calls.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-test-support --lib fixtures
```

Expected: FAIL because the fixture function is absent.

- [ ] **Step 3: Implement fixture**

Implement `sine_fixture_16khz_mono()` using `std::f32::consts::PI`. Generate exactly one second of 440 Hz mono `f32` samples at 16 kHz into `AudioSegment.samples_f32_mono`.

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-test-support --lib fixtures
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolect-test-support
git commit -m "test: add deterministic audio fixtures"
```

## Task 3: Fixture Audio Adapter

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-adapter-fixture-audio/Cargo.toml`
- Create: `crates/idiolect-adapter-fixture-audio/src/lib.rs`

- [ ] **Step 1: Write failing adapter tests**

Add tests `fixture_audio_requires_start_before_stop` and `fixture_audio_stop_returns_fixture_segment_after_start`. The first expects `FixtureAudioError::NotStarted`. The second starts capture, stops capture, and asserts `sample_rate_hz == 16000`, `channels == 1`, and sample length `16000`.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-adapter-fixture-audio --lib
```

Expected: FAIL because `FixtureAudio` is absent.

- [ ] **Step 3: Implement adapter**

Implement `FixtureAudio` with a `BTreeSet<ImeSessionId>` of started sessions. `start_capture` inserts the session. `stop_capture` returns `FixtureAudioError::NotStarted` if absent; otherwise it removes the session and returns `sine_fixture_16khz_mono()`.

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-adapter-fixture-audio --lib
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolect-adapter-fixture-audio
git commit -m "feat: add fixture audio adapter"
```

## Task 4: Fixture ASR Adapter

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-adapter-fixture-asr/Cargo.toml`
- Create: `crates/idiolect-adapter-fixture-asr/src/lib.rs`

- [ ] **Step 1: Write failing ASR tests**

Add tests `fixture_asr_returns_configured_transcript_and_metadata` and `fixture_asr_reports_capabilities`. The transcript test uses `FixtureAsr::new("restart traffic")`, transcribes the sine fixture, and asserts text, engine name `fixture-asr`, engine version `0.1.0`, and confidence `Some(1.0)`.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-adapter-fixture-asr --lib
```

Expected: FAIL because `FixtureAsr` is absent.

- [ ] **Step 3: Implement adapter**

Implement `FixtureAsr { transcript: String }` with `new<S: Into<String>>(transcript: S)`. The `AsrPort` implementation returns fixed capabilities and a `TranscriptDraft` whose text exactly equals the configured transcript.

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-adapter-fixture-asr --lib
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolect-adapter-fixture-asr
git commit -m "feat: add fixture asr adapter"
```

## Task 5: Fixture Codec Adapter

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-adapter-fixture-codec/Cargo.toml`
- Create: `crates/idiolect-adapter-fixture-codec/src/lib.rs`

- [ ] **Step 1: Write failing codec tests**

Add tests `fixture_codec_round_trips_segment` and `fixture_codec_rejects_corrupt_payload`. The round-trip test asserts sample rate, channel count, duration, and sample vector equality. The corrupt-payload test truncates payload and expects `FixtureCodecError::CorruptPayload`.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-adapter-fixture-codec --lib
```

Expected: FAIL because `FixtureCodec` is absent.

- [ ] **Step 3: Implement adapter**

Use a fixture-local bytes format:

```text
magic bytes IDFX1
sample_rate_hz little-endian u32
channels little-endian u16
duration_ms little-endian u32
sample_count little-endian u32
sample bytes as little-endian f32 values
```

`decode` rejects wrong magic, short headers, inconsistent byte length, and non-`fixture-codec` codec names with `FixtureCodecError::CorruptPayload`.

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-adapter-fixture-codec --lib
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolect-adapter-fixture-codec
git commit -m "feat: add fixture codec adapter"
```

## Task 6: Cross-Adapter Fixture Integration

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-integration-tests/Cargo.toml`
- Create: `crates/idiolect-integration-tests/tests/fixture_audio_asr_codec_contracts.rs`

- [ ] **Step 1: Write failing integration test**

Create `fixture_pipeline_contract_is_deterministic_end_to_end`. It constructs `FixtureAudio`, `FixtureAsr::new("restart traffic")`, and `FixtureCodec`, captures fixture audio, transcribes it, encodes it, decodes it, and asserts transcript text plus decoded sample vector equality.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-integration-tests --test fixture_audio_asr_codec_contracts
```

Expected: FAIL until the integration crate depends on the three fixture adapter crates.

- [ ] **Step 3: Wire dependencies**

Add path dependencies for `idiolect-adapter-fixture-audio`, `idiolect-adapter-fixture-asr`, `idiolect-adapter-fixture-codec`, `idiolect-common`, and `idiolect-ports`.

- [ ] **Step 4: Run green command and scope scan**

```bash
cargo test -p idiolect-integration-tests --test fixture_audio_asr_codec_contracts
rg -n "cpal|silero|whisper|opus|onnx|pytorch|peft|python" crates/idiolect-adapter-fixture-audio crates/idiolect-adapter-fixture-asr crates/idiolect-adapter-fixture-codec crates/idiolect-test-support
bash ci/scripts/test-rust.sh
```

Expected: integration test passes, scan emits no prohibited backend names, and Rust gate passes with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolect-integration-tests
git commit -m "test: add fixture audio asr codec integration contracts"
```

## Rejection Criteria

Reject and rework this child if any condition holds:

```text
fixture adapters import real backend crates
AudioSegment field names diverge from this plan without parent-plan amendment
fixture ASR text changes based on audio contents
fixture codec silently accepts corrupt payloads
cross-adapter fixture test is absent
any lint, compile, doc, or test warning appears
```

