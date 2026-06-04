# Idiolect 05 Real Adapters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add real CPAL audio, Opus codec, VAD, and Whisper ASR adapters behind stable ports, with dependency decisions, contract tests, and backend-leakage guards.

**Architecture:** Real adapters live only in adapter crates. Each adapter has private backend details, public constructors that return Idiolect-owned errors, and port implementations that expose only Idiolect-owned DTOs.

**Tech Stack:** Rust, CPAL, Opus-compatible Rust binding selected by decision record, Rust VAD runtime `webrtc-vad` selected by decision record, `whisper-rs`, fixture assets for deterministic tests, strict Cargo lint gates.

---

## Scope Boundary

Allowed behavior:

```text
real adapter dependency decisions
real adapter crates behind stable ports
compile and contract tests for CPAL audio adapter
real Opus encode/decode test using sine fixture
real VAD segmentation test using fixture audio
real Whisper transcription test using repository-managed fixture model and audio
backend-leakage and required-path dependency guard CI scripts
```

Forbidden behavior:

```text
third-party backend types in core, ports, or application public APIs
Python required-path code
hardware-only tests as the only proof of adapter behavior
network download during normal cargo test
warning suppression
wildcard dependencies
```

Required gates after every code task:

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

Real adapter fixture rule: if a real backend requires a model or binary fixture, the fixture must be repository-managed or fetched by a deterministic script with a pinned URL and SHA-256. The acceptance gate must run the real backend test before this child is complete.

## Task 1: Dependency Policy And Backend-Leakage Scripts

**Owner:** Gatekeeper-local for dependency records, Spark worker allowed for scripts  
**Model:** Gatekeeper-local for dependency choices; `gpt-5.3-codex-spark` for scripts  
**Files:**

- Create: `docs/decisions/0004-cpal-audio-adapter.md`
- Create: `docs/decisions/0005-opus-codec-adapter.md`
- Create: `docs/decisions/0006-vad-adapter.md`
- Create: `docs/decisions/0007-whisper-asr-adapter.md`
- Create: `ci/scripts/test-real-adapter-deps.sh`
- Create: `ci/scripts/test-interface-no-backend-leakage.sh`

- [x] **Step 1: Record current dependency choices**

Run current-version lookup commands before manifest edits:

```bash
cargo search cpal --limit 1
cargo search whisper-rs --limit 1
cargo search opus --limit 5
cargo search webrtc-vad --limit 1
```

For each decision record, include exact crate version, selected features, native system dependencies, fixture strategy, port-isolation reason, and rollback path if the dependency blocks zero-warning gates. If a dependency cannot be selected with exact version and test strategy, return `NEEDS_CONTEXT` and do not edit manifests.

- [x] **Step 2: Create dependency guard script**

Create `ci/scripts/test-real-adapter-deps.sh` to run `cargo metadata --format-version 1 --no-deps`, enumerate workspace manifest paths, and fail if any manifest `version =` requirement is empty, `*`, starts with `^`, or starts with `~`.

- [x] **Step 3: Create backend leakage script**

Create `ci/scripts/test-interface-no-backend-leakage.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

if rg -n "\bcpal\b|\bwhisper\b|\bsilero\b|fast-vad|webrtc-vad|\bwebrtc\b|\bfvad\b|\blibfvad\b|\bopus\b|\bonnx\b|\bort\b|\brusqlite\b|\bpytorch\b|\bpeft\b|\bpython\b" \
  crates/idiolect-core crates/idiolect-ports crates/idiolect-application; then
  echo "backend implementation detail leaked into interface crates" >&2
  exit 1
fi
```

- [x] **Step 4: Run script gates**

```bash
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [x] **Step 5: Commit**

```bash
git add docs/decisions/0004-cpal-audio-adapter.md docs/decisions/0005-opus-codec-adapter.md docs/decisions/0006-vad-adapter.md docs/decisions/0007-whisper-asr-adapter.md ci/scripts/test-real-adapter-deps.sh ci/scripts/test-interface-no-backend-leakage.sh
git commit -m "chore: add real adapter dependency and leakage gates"
```

## Task 2: CPAL Audio Adapter

**Owner:** Stronger model or gatekeeper-local for adapter boundary; Spark worker can implement tests after boundary is approved  
**Model:** `gpt-5.4-mini` or gatekeeper-local for design review, then `gpt-5.3-codex-spark` for mechanical implementation  
**Files:**

- Create: `crates/idiolect-adapter-cpal/Cargo.toml`
- Create: `crates/idiolect-adapter-cpal/src/lib.rs`
- Modify: `Cargo.toml`
- Create: `crates/idiolect-integration-tests/tests/real_audio_adapter_contracts.rs`

- [x] **Step 1: Write failing adapter tests**

Create tests `stop_before_start_returns_not_started` and `missing_device_is_reported_as_typed_error`. The missing-device test calls `CpalAudioInput::open_device_by_name("__idiolect_missing_device__")` and expects `CpalAudioInputError::DeviceNotFound`.

- [x] **Step 2: Run red command**

```bash
cargo test -p idiolect-adapter-cpal --lib
```

Expected: FAIL because the crate is absent.

- [x] **Step 3: Implement CPAL adapter behind private backend trait**

Implementation requirements:

```text
CpalAudioInput implements AudioInputPort
public errors use CpalAudioInputError only
private trait CaptureBackend hides cpal concrete stream and device types
new_for_test accepts a test backend and is available to tests only
open_default and open_device_by_name are public constructors
no automated capture test depends on a real default device
```

- [x] **Step 4: Add integration contract test**

Create `real_audio_adapter_contracts.rs` with `cpal_missing_named_device_is_deterministic`, asserting the same typed missing-device error through public exports.

- [x] **Step 5: Run green command and gates**

```bash
cargo test -p idiolect-adapter-cpal --lib
cargo test -p idiolect-integration-tests --test real_audio_adapter_contracts
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [x] **Step 6: Commit**

```bash
git add Cargo.toml crates/idiolect-adapter-cpal crates/idiolect-integration-tests/tests/real_audio_adapter_contracts.rs
git commit -m "feat: add cpal audio adapter boundary"
```

## Task 3: Real Opus Codec Adapter

**Owner:** Stronger model or gatekeeper-local for dependency behavior; Spark worker can implement contract tests  
**Model:** `gpt-5.4-mini` or gatekeeper-local, then `gpt-5.3-codex-spark`  
**Files:**

- Create: `crates/idiolect-adapter-opus/Cargo.toml`
- Create: `crates/idiolect-adapter-opus/src/lib.rs`
- Modify: `Cargo.toml`
- Create: `crates/idiolect-integration-tests/tests/real_codec_contracts.rs`

- [x] **Step 1: Write failing codec tests**

Create `opus_codec_round_trips_fixture_metadata`: encode and decode `sine_fixture_16khz_mono()`, assert `encoded.codec_name == "opus"`, decoded sample rate `16000`, one channel, matching duration, and matching sample count.

- [x] **Step 2: Run red command**

```bash
cargo test -p idiolect-adapter-opus --lib
```

Expected: FAIL because the crate is absent.

- [x] **Step 3: Implement codec**

Requirements:

```text
OpusCodec implements AudioCodecPort
unsupported sample rate returns typed error before calling backend
decode rejects non-opus EncodedAudio.codec_name
fixture round trip preserves metadata and sample count
public API exposes no third-party Opus types
```

- [x] **Step 4: Add integration contract**

Create `real_codec_contracts.rs` with the same round-trip metadata assertion through public crate exports.

- [x] **Step 5: Run green command and gates**

```bash
cargo test -p idiolect-adapter-opus --lib
cargo test -p idiolect-integration-tests --test real_codec_contracts
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [x] **Step 6: Commit**

```bash
git add Cargo.toml crates/idiolect-adapter-opus crates/idiolect-integration-tests/tests/real_codec_contracts.rs
git commit -m "feat: add opus codec adapter"
```

## Task 4: Real VAD Adapter

**Owner:** Gatekeeper-local for runtime selection, Spark worker for mechanical tests after decision  
**Model:** Gatekeeper-local or `gpt-5.4-mini`, then `gpt-5.3-codex-spark`  
**Files:**

- Create: `crates/idiolect-adapter-vad/Cargo.toml`
- Create: `crates/idiolect-adapter-vad/src/lib.rs`
- Modify: `Cargo.toml`
- Create: `crates/idiolect-integration-tests/tests/real_vad_contracts.rs`
- Modify: `crates/idiolect-test-support/src/fixtures.rs`

- [x] **Step 1: Write failing VAD tests**

Create `vad_segments_fixture_into_speech_regions`: load `speech_and_silence_fixture_16khz_mono()`, construct `VadAdapter::new()`, segment the fixture, assert exactly one speech segment, sample rate `16000`, and duration at least `400` ms.

- [x] **Step 2: Run red command**

```bash
cargo test -p idiolect-adapter-vad --lib
```

Expected: FAIL because the adapter crate and fixture helper are absent.

- [x] **Step 3: Implement VAD adapter and fixture**

Requirements:

```text
VadAdapter implements VadPort
VadAdapter::new() initializes webrtc-vad for 16 kHz mono frame detection
segment returns deterministic speech slices for the fixture
public API exposes no ONNX, Silero, ort, WebRTC, libfvad, FFI, or runtime-specific types
speech_and_silence_fixture_16khz_mono is pure Rust test-support data
```

- [x] **Step 4: Add integration contract**

Create `real_vad_contracts.rs` asserting `VadAdapter::new().segment(&speech_and_silence_fixture_16khz_mono())` returns one segment.

- [x] **Step 5: Run green command and gates**

```bash
cargo test -p idiolect-adapter-vad --lib
cargo test -p idiolect-integration-tests --test real_vad_contracts
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [x] **Step 6: Commit**

```bash
git add Cargo.toml crates/idiolect-adapter-vad crates/idiolect-integration-tests/tests/real_vad_contracts.rs crates/idiolect-test-support/src/fixtures.rs
git commit -m "feat: add vad adapter contract"
```

## Task 5: Real Whisper ASR Adapter

**Owner:** Gatekeeper-local for fixture model strategy; stronger model for backend boundary; Spark worker for implementation after strategy is accepted  
**Model:** Gatekeeper-local or `gpt-5.4-mini`, then `gpt-5.3-codex-spark`  
**Files:**

- Create: `crates/idiolect-adapter-whisper/Cargo.toml`
- Create: `crates/idiolect-adapter-whisper/src/lib.rs`
- Modify: `Cargo.toml`
- Create: `crates/idiolect-integration-tests/tests/real_asr_contracts.rs`
- Create: `tests/fixtures/whisper/README.md`
- Create: `ci/scripts/fetch-whisper-fixture.sh`

- [x] **Step 1: Establish fixture model artifact**

Create `tests/fixtures/whisper/README.md` with model file name, pinned download URL, SHA-256 digest, license note, and expected transcript for `tests/fixtures/audio/restart_traffic_16khz_mono.wav`. Create `ci/scripts/fetch-whisper-fixture.sh` that downloads the exact model file, verifies SHA-256, and writes it to `tests/fixtures/whisper/`. The normal test command must not perform a network download; the fixture must already be present or the worker returns `NEEDS_CONTEXT` before claiming completion.

- [x] **Step 2: Write failing Whisper tests**

Create tests `whisper_transcribes_fixture_audio` and `whisper_reports_capabilities_without_backend_type_leakage`. The transcription test uses `WhisperAsr::load_fixture_model()`, transcribes `restart_traffic_fixture_16khz_mono()`, asserts the lowercase text contains `restart` and `traffic`, and metadata engine name is `whisper-rs`.

- [x] **Step 3: Run red command**

```bash
cargo test -p idiolect-adapter-whisper --lib
```

Expected: FAIL because the crate and fixture model are absent.

- [x] **Step 4: Implement adapter**

Requirements:

```text
WhisperAsr implements AsrPort
load_fixture_model loads the repository-managed model path only
transcribe converts AudioSegment to backend input privately
public API exposes no whisper-rs concrete type
metadata contains engine_name whisper-rs and exact dependency version
missing model returns typed MissingFixtureModel error
corrupt model files return typed load errors and never panic
```

- [x] **Step 5: Add integration contract**

Create `real_asr_contracts.rs` asserting the real Whisper adapter transcribes fixture audio and includes `restart` plus `traffic`.

- [x] **Step 6: Run green command and gates**

```bash
cargo test -p idiolect-adapter-whisper --lib
cargo test -p idiolect-integration-tests --test real_asr_contracts
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings and a real Whisper backend exercised by the fixture model.

- [x] **Step 7: Commit**

```bash
git add Cargo.toml crates/idiolect-adapter-whisper crates/idiolect-integration-tests/tests/real_asr_contracts.rs crates/idiolect-test-support tests/fixtures/whisper ci/scripts/fetch-whisper-fixture.sh
git commit -m "feat: add whisper asr adapter contract"
```

## Task 6: Real Adapter Contract Matrix

**Owner:** Gatekeeper-local for acceptance  
**Model:** Gatekeeper-local  
**Files:**

- Create: `crates/idiolect-integration-tests/tests/real_adapter_contracts.rs`

- [x] **Step 1: Write full real adapter matrix test**

Create `real_adapter_matrix_processes_fixture_audio`: load restart-traffic fixture, encode/decode through Opus, segment through VAD, transcribe first segment through Whisper, and assert transcript contains `restart` and `traffic`.

- [x] **Step 2: Run red command**

```bash
cargo test -p idiolect-integration-tests --test real_adapter_contracts
```

Expected: FAIL until all real adapters and fixtures are wired.

- [x] **Step 3: Run green command and full gates**

```bash
cargo test -p idiolect-integration-tests --test real_adapter_contracts
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [x] **Step 4: Commit**

```bash
git add crates/idiolect-integration-tests/tests/real_adapter_contracts.rs
git commit -m "test: add real adapter fixture matrix"
```

## Rejection Criteria

Reject and rework this child if any condition holds:

```text
any real adapter dependency lacks an exact version and decision record
Python-related crates appear in required dependency paths
any real adapter exposes backend concrete types in public APIs
real Whisper or VAD tests are replaced by fake adapter tests
network is required during cargo test
CPAL tests require local audio hardware to pass
any model fixture lacks SHA-256 verification
backend-leakage script reports a match
any lint, compile, doc, or test warning appears
```

