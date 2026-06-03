# Idiolect 00 Readiness Workspace Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the implementation-ready documentation amendments, Rust workspace baseline, common types, core session rules, and first port contracts as a compilable lint-clean foundation.

**Architecture:** This child plan implements the inner foundation: readiness docs, the lint-clean Cargo workspace, `idiolect-common`, `idiolect-core`, `idiolect-ports`, and `idiolect-test-support`. It creates placeholder crates needed by the workspace but does not implement real adapters, daemon workflows, SQLite storage, IPC sockets, Fcitx5, ASR, VAD, Opus, or packaging.

**Tech Stack:** Rust 1.96.0 stable pinned by `rust-toolchain.toml`, Cargo workspace lints, Serde, thiserror, time, uuid, strict `-D warnings`, and fake contract adapters only.

---

## Scope Boundary

Allowed implementation behavior:

```text
readiness documentation corrections
workspace bootstrap
common IDs and protocol DTOs
core IME session and training-candidate rules
first port traits
fake input method contract harness
```

Forbidden implementation behavior:

```text
real audio capture
real ASR
real VAD
real codec
real SQLite storage
Tokio socket server/client
Fcitx5 C++ shim
Python required-path code
packaging scripts that require artifacts not yet produced
```

Required gates after every code task:

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

## Approved Execution Amendment

The user approved implementation in the current `main` checkout for this plan on 2026-06-04. Agents must still avoid unrelated file changes, inspect `git status` before edits, and keep commits scoped by task.

Rust is pinned to the latest stable release verified from the official Rust blog: `1.96.0`, released 2026-05-28. Rust has no separate LTS channel for this project. If `1.96.0` is not installed locally, install it before running workspace gates.

## Task 1: Master Plan Readiness Amendments

**Owner:** Gatekeeper  
**Model:** Gatekeeper-local or `gpt-5.4`; do not assign final decisions to Spark  
**Files:**

- Create: `docs/decisions/0001-rust-first-v1-architecture.md`
- Create: `docs/implementation/v1-readiness-amendments.md`
- Modify: `.gitignore`
- Modify: `docs/idiolect_master_plan_rust_first.md`

- [ ] **Step 1: Ensure project-local worktree ignores are present**

Edit `.gitignore` so project-local worktrees are ignored:

```gitignore
.worktrees/
```

Run:

```bash
git check-ignore -q .worktrees/
```

Expected: exits 0.

- [ ] **Step 2: Write the readiness amendment**

Create `docs/implementation/v1-readiness-amendments.md` with this content:

```markdown
# V1 Readiness Amendments

## Required Corrections Before Source Implementation

1. Python is research-only and is excluded from required v1 runtime, tests, training, promotion, rollback, and packaging.
2. Rust contract tests are the required adapter contract mechanism.
3. The authoritative crate topology is `common`, `core`, `ports`, `application`, adapter crates, `idiolectd`, `idiolect-cli`, `test-support`, and `integration-tests`.
4. SQLite storage uses append-only `event_log` plus materialized tables from the first implementation.
5. Adapter promotion requires artifact compatibility metadata and cannot rely on model-quality metrics alone.
6. All dependencies must use pinned versions. Wildcard dependency versions are rejected.
7. Lint warnings are errors. Rust uses `-D warnings`; C++ uses `-Werror`.
8. Work may proceed on `main` for this implementation because the user explicitly approved it on 2026-06-04.
9. Rust is pinned to stable `1.96.0`, released 2026-05-28. Rust has no separate LTS channel for this project.
10. Non-Rust and third-party backends must only appear behind Idiolect-owned interfaces; backend-specific types must not leak into `idiolect-core`, `idiolect-ports`, or `idiolect-application` public APIs.
```

- [ ] **Step 3: Write the architecture decision record**

Create `docs/decisions/0001-rust-first-v1-architecture.md`:

```markdown
# Decision 0001: Rust-First V1 Architecture

Status: Accepted

Idiolect v1 uses Rust for runtime, orchestration, storage, classification, manifest generation, evaluation, promotion, rollback, and required tests. Python may exist only as research reference material under `research/` and is not part of required product operation.

The v1 workspace is split into `idiolect-common`, `idiolect-core`, `idiolect-ports`, `idiolect-application`, adapter crates, `idiolectd`, `idiolect-cli`, `idiolect-test-support`, and `idiolect-integration-tests`.

Rust is pinned to stable `1.96.0`, released 2026-05-28. Rust has no separate LTS channel for this project. Work may proceed in the current `main` checkout because the user explicitly approved that execution mode on 2026-06-04.

Consequences:

- Core crates never expose Fcitx5, whisper-rs, Silero, Opus, rusqlite, Burn, Candle, ONNX Runtime, PyTorch, PEFT, Python, or other backend-specific types.
- Required contract tests are Rust tests.
- The Fcitx5 engine remains a thin C++ shim and communicates through versioned IPC.
- Adapter promotion requires artifact compatibility, metrics, and rollback evidence.
- Non-Rust and third-party backend integrations are adapters only and must communicate through Idiolect-owned interfaces.
```

- [ ] **Step 4: Remove contradictions from the master plan**

Edit `docs/idiolect_master_plan_rust_first.md` so required-path references to Python become research-only. Replace required `python-peft` config examples with:

```toml
[trainer]
backend = "rust-native-lora"
auto_train = false
```

Replace `trainer_contract.py` and `evaluator_contract.py` examples with:

```text
tests/contracts/
  trainer_contract.rs
  evaluator_contract.rs
```

Replace wildcard dependency examples with pinned-version examples. If exact versions need verification, add no dependency line until a later implementation task verifies the current crate version.

- [ ] **Step 5: Verify amendment**

Run:

```bash
rg -n "python-peft|python_trainer|trainer_contract\\.py|evaluator_contract\\.py|pytest for Rust trainer|= \"\\*\"" docs/idiolect_master_plan_rust_first.md docs/implementation/v1-readiness-amendments.md docs/decisions/0001-rust-first-v1-architecture.md
```

Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add .gitignore docs/idiolect_master_plan_rust_first.md docs/implementation/v1-readiness-amendments.md docs/decisions/0001-rust-first-v1-architecture.md
git commit -m "docs: lock rust-first v1 implementation decisions"
```

## Task 2: Workspace Bootstrap And Lint Baseline

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `.cargo/config.toml`
- Create: `crates/*/Cargo.toml`
- Create: `crates/*/src/lib.rs`
- Create: `ci/scripts/test-rust.sh`
- Create: `README.md`

- [ ] **Step 1: Verify the repository has no workspace**

Run:

```bash
cargo metadata --format-version 1
```

Expected: fails with a missing manifest error because the workspace has not been created.

- [ ] **Step 2: Add the root workspace manifest**

Create `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = [
  "crates/idiolect-common",
  "crates/idiolect-core",
  "crates/idiolect-ports",
  "crates/idiolect-application",
  "crates/idiolect-ipc",
  "crates/idiolect-adapter-memory",
  "crates/idiolect-adapter-sqlite",
  "crates/idiolect-adapter-fixture-audio",
  "crates/idiolect-adapter-fixture-asr",
  "crates/idiolect-adapter-fixture-codec",
  "crates/idiolect-trainerctl",
  "crates/idiolectd",
  "crates/idiolect-cli",
  "crates/idiolect-test-support",
  "crates/idiolect-integration-tests",
]

[workspace.package]
edition = "2021"
license = "AGPL-3.0-only"
rust-version = "1.96"

[workspace.lints.rust]
warnings = "deny"
unsafe_code = "forbid"
rust_2018_idioms = "deny"
unused_lifetimes = "deny"
unreachable_pub = "deny"

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }
correctness = { level = "deny", priority = -1 }
suspicious = { level = "deny", priority = -1 }
complexity = { level = "deny", priority = -1 }
perf = { level = "deny", priority = -1 }
style = { level = "deny", priority = -1 }
dbg_macro = "deny"
todo = "deny"
unimplemented = "deny"

[workspace.dependencies]
serde = { version = "1.0.203", features = ["derive"] }
serde_json = "1.0.117"
thiserror = "1.0.61"
time = { version = "0.3.36", features = ["serde", "formatting", "parsing"] }
tokio = { version = "1.38.0", features = ["rt-multi-thread", "macros", "net", "io-util", "sync", "time"] }
uuid = { version = "1.8.0", features = ["v4", "serde"] }
```

- [ ] **Step 3: Pin the Rust toolchain and Cargo behavior**

Create `rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.96.0"
components = ["cargo", "clippy", "rustfmt"]
profile = "minimal"
```

Create `.cargo/config.toml`:

```toml
[build]
rustflags = ["-D", "warnings"]
rustdocflags = ["-D", "warnings"]

[term]
color = "auto"
```

- [ ] **Step 4: Add minimal crate manifests**

Each crate manifest must include workspace lints:

```toml
[package]
name = "idiolect-common"
version = "0.1.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
serde.workspace = true
thiserror.workspace = true
time.workspace = true
uuid.workspace = true
```

For each other crate, change `name` and add only the workspace dependencies it uses. A crate with no dependencies must still include:

```toml
[lints]
workspace = true
```

- [ ] **Step 5: Add empty library files that compile**

Each library crate starts with:

```rust
//! Crate documentation for the Idiolect workspace.

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
```

Binary crates use a library plus a small `main.rs`:

```rust
fn main() {
    println!("{}", idiolect_cli::crate_name());
}
```

- [ ] **Step 6: Add CI Rust gate script**

Create `ci/scripts/test-rust.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

- [ ] **Step 7: Verify lint baseline**

Run:

```bash
bash ci/scripts/test-rust.sh
```

Expected: all commands pass with zero warnings.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml rust-toolchain.toml .cargo/config.toml crates README.md ci/scripts/test-rust.sh
git commit -m "chore: bootstrap lint-clean rust workspace"
```

## Task 3: Common IDs, Time, Config, Errors, And Protocol DTOs

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-common/src/lib.rs`
- Create: `crates/idiolect-common/src/ids.rs`
- Create: `crates/idiolect-common/src/time.rs`
- Create: `crates/idiolect-common/src/config.rs`
- Create: `crates/idiolect-common/src/error.rs`
- Create: `crates/idiolect-common/src/protocol.rs`

- [ ] **Step 1: Write failing serde and config tests**

Add tests in `crates/idiolect-common/src/ids.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{ImeSessionId, UserId};

    #[test]
    fn ime_session_id_round_trips_through_json() {
        let id = ImeSessionId::new();
        let encoded = serde_json::to_string(&id).expect("session id should serialize");
        let decoded: ImeSessionId =
            serde_json::from_str(&encoded).expect("session id should deserialize");
        assert_eq!(decoded, id);
    }

    #[test]
    fn default_user_id_is_stable() {
        assert_eq!(UserId::default_user().as_str(), "default");
    }
}
```

Run:

```bash
cargo test -p idiolect-common --lib
```

Expected: fails because `ImeSessionId` and `UserId` do not exist.

- [ ] **Step 2: Implement IDs**

Implement `ids.rs`:

```rust
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ImeSessionId(Uuid);

impl ImeSessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ImeSessionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct UserId(String);

impl UserId {
    #[must_use]
    pub fn default_user() -> Self {
        Self("default".to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
```

- [ ] **Step 3: Add protocol DTO tests**

In `protocol.rs`, add a failing test for version negotiation:

```rust
#[cfg(test)]
mod tests {
    use super::{ClientHello, ServerHello};

    #[test]
    fn hello_messages_round_trip_with_protocol_version() {
        let hello = ClientHello {
            client_name: "idiolect-fcitx5".to_owned(),
            protocol_version: 1,
            features: vec!["preedit".to_owned(), "commit".to_owned()],
        };
        let json = serde_json::to_string(&hello).expect("hello should serialize");
        let decoded: ClientHello = serde_json::from_str(&json).expect("hello should deserialize");
        assert_eq!(decoded.protocol_version, 1);

        let ack = ServerHello {
            protocol_version: 1,
            accepted_features: vec!["preedit".to_owned()],
        };
        assert_eq!(ack.accepted_features, ["preedit"]);
    }
}
```

Run:

```bash
cargo test -p idiolect-common hello_messages_round_trip_with_protocol_version
```

Expected: fails because protocol DTOs do not exist.

- [ ] **Step 4: Implement protocol DTOs and module exports**

Add DTOs:

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientHello {
    pub client_name: String,
    pub protocol_version: u16,
    pub features: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerHello {
    pub protocol_version: u16,
    pub accepted_features: Vec<String>,
}
```

Update `lib.rs`:

```rust
//! Shared Idiolect types that do not depend on backend libraries.

pub mod config;
pub mod error;
pub mod ids;
pub mod protocol;
pub mod time;
```

- [ ] **Step 5: Verify**

Run:

```bash
cargo test -p idiolect-common
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-common
git commit -m "feat: add common ids and protocol dto types"
```

## Task 4: Core Session State And Candidate Rules

**Owner:** Spark worker allowed, gatekeeper reviews state semantics  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-core/src/lib.rs`
- Create: `crates/idiolect-core/src/domain/session.rs`
- Create: `crates/idiolect-core/src/domain/candidate.rs`
- Create: `crates/idiolect-core/src/domain/events.rs`
- Create: `crates/idiolect-core/src/rules/session_lifecycle.rs`

- [ ] **Step 1: Write failing state transition tests**

In `session_lifecycle.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::domain::session::{ImeSession, ImeSessionState};

    #[test]
    fn committed_session_cannot_be_cancelled() {
        let session = ImeSession::new_for_test()
            .recording_started()
            .transcription_started()
            .preedit_started("restart traffic")
            .committed("restart Traefik");

        let result = session.try_cancel();

        assert!(result.is_err());
        assert_eq!(session.state(), ImeSessionState::Committed);
    }

    #[test]
    fn duplicate_commit_is_idempotent() {
        let session = ImeSession::new_for_test()
            .recording_started()
            .transcription_started()
            .preedit_started("restart traffic")
            .committed("restart Traefik");

        let result = session.try_commit("restart Traefik");

        assert!(result.is_ok());
        assert_eq!(session.state(), ImeSessionState::Committed);
    }
}
```

Run:

```bash
cargo test -p idiolect-core --lib
```

Expected: fails because session domain does not exist.

- [ ] **Step 2: Implement minimal session state machine**

Implement states:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImeSessionState {
    Created,
    Recording,
    Transcribing,
    PreeditActive,
    Committed,
    Cancelled,
    Abandoned,
}
```

Implement `ImeSession` with these exact public signatures so later workers do not improvise incompatible ownership semantics:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImeSession {
    state: ImeSessionState,
    raw_stt_text: Option<String>,
    committed_text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionTransitionError {
    AlreadyCommitted,
    AlreadyCancelled,
    InvalidTransition {
        from: ImeSessionState,
        action: &'static str,
    },
}

impl ImeSession {
    #[must_use]
    pub fn new_for_test() -> Self;

    #[must_use]
    pub fn state(&self) -> ImeSessionState;

    #[must_use]
    pub fn recording_started(self) -> Self;

    #[must_use]
    pub fn transcription_started(self) -> Self;

    #[must_use]
    pub fn preedit_started(self, raw_stt_text: &str) -> Self;

    #[must_use]
    pub fn committed(self, committed_text: &str) -> Self;

    pub fn try_commit(&self, committed_text: &str) -> Result<Self, SessionTransitionError>;

    pub fn try_cancel(&self) -> Result<Self, SessionTransitionError>;
}
```

The builder-style methods consume `self`; fallible duplicate/late-event methods borrow `&self` so tests can assert the original state remains unchanged after a rejected transition.

- [ ] **Step 3: Write failing candidate rule tests**

In `candidate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{CaptureQuality, TrainingCandidate, TrainingCandidateSource};

    #[test]
    fn preedit_correction_creates_high_quality_candidate() {
        let candidate = TrainingCandidate::from_preedit_correction(
            "restart traffic",
            "restart Traefik",
        )
        .expect("changed preedit should create candidate");

        assert_eq!(candidate.source(), TrainingCandidateSource::ImePreeditCorrection);
        assert_eq!(candidate.capture_quality(), CaptureQuality::High);
        assert_eq!(candidate.trust_score(), 1.0);
    }

    #[test]
    fn accepted_without_edit_creates_weak_candidate() {
        let candidate = TrainingCandidate::from_acceptance("deploy the container");

        assert_eq!(candidate.source(), TrainingCandidateSource::AcceptedWithoutEdit);
        assert_eq!(candidate.capture_quality(), CaptureQuality::Low);
        assert_eq!(candidate.trust_score(), 0.6);
    }
}
```

Run:

```bash
cargo test -p idiolect-core --lib
```

Expected: fails because candidate domain does not exist.

- [ ] **Step 4: Implement candidate rules**

Implement the exact source, quality, and trust-score mappings tested above. Use `f32` for trust score and compare exact constants only for these fixed values.

- [ ] **Step 5: Verify no backend leakage**

Run:

```bash
rg -n "fcitx|whisper|silero|opus|rusqlite|python|pytorch|peft|burn|candle|onnx" crates/idiolect-core
```

Expected: no output.

- [ ] **Step 6: Verify**

```bash
cargo test -p idiolect-core
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/idiolect-core
git commit -m "feat: add core session and candidate rules"
```

## Task 5: Ports And Rust Contract Test Harness

**Owner:** Spark worker allowed for traits, gatekeeper reviews boundaries  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-ports/src/lib.rs`
- Create: `crates/idiolect-ports/src/input_method.rs`
- Create: `crates/idiolect-ports/src/audio.rs`
- Create: `crates/idiolect-ports/src/vad.rs`
- Create: `crates/idiolect-ports/src/asr.rs`
- Create: `crates/idiolect-ports/src/codec.rs`
- Create: `crates/idiolect-ports/src/storage.rs`
- Create: `crates/idiolect-ports/src/trainer.rs`
- Create: `crates/idiolect-ports/src/evaluator.rs`
- Create: `crates/idiolect-ports/src/adapter_registry.rs`
- Modify: `crates/idiolect-test-support/src/lib.rs`
- Create: `crates/idiolect-test-support/src/fakes.rs`

- [ ] **Step 1: Write failing fake input method contract test**

In `idiolect-test-support/src/fakes.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::FakeInputMethod;
    use idiolect_common::ids::ImeSessionId;
    use idiolect_ports::input_method::InputMethodPort;

    #[test]
    fn input_method_records_preedit_before_commit() {
        let mut input = FakeInputMethod::default();
        let session_id = ImeSessionId::new();

        input.show_preedit(session_id, "restart traffic").expect("preedit should show");
        input.commit_text(session_id, "restart Traefik").expect("commit should succeed");

        assert_eq!(input.events(), ["show_preedit:restart traffic", "commit:restart Traefik"]);
    }
}
```

Run:

```bash
cargo test -p idiolect-test-support input_method_records_preedit_before_commit
```

Expected: fails because the trait and fake do not exist.

- [ ] **Step 2: Implement `InputMethodPort` and fake**

Define:

```rust
use idiolect_common::ids::ImeSessionId;

pub trait InputMethodPort {
    type Error;

    fn show_preedit(&mut self, session_id: ImeSessionId, text: &str) -> Result<(), Self::Error>;
    fn update_preedit(&mut self, session_id: ImeSessionId, text: &str) -> Result<(), Self::Error>;
    fn commit_text(&mut self, session_id: ImeSessionId, text: &str) -> Result<(), Self::Error>;
    fn cancel_preedit(&mut self, session_id: ImeSessionId) -> Result<(), Self::Error>;
}
```

Implement `FakeInputMethod` with an internal `Vec<String>` event log and an `events(&self) -> Vec<&str>` accessor.

- [ ] **Step 3: Write failing capability test**

Add:

```rust
#[cfg(test)]
mod capability_tests {
    use idiolect_ports::asr::AdapterCapabilities;

    #[test]
    fn adapter_capabilities_report_stable_name_and_version() {
        let capabilities = AdapterCapabilities {
            name: "fixture-asr".to_owned(),
            version: "0.1.0".to_owned(),
            supports_streaming: false,
            supports_word_timestamps: false,
            supports_confidence: true,
            supports_gpu: false,
            supports_incremental_updates: false,
        };

        assert_eq!(capabilities.name, "fixture-asr");
        assert!(!capabilities.supports_gpu);
    }
}
```

Run:

```bash
cargo test -p idiolect-ports adapter_capabilities_report_stable_name_and_version
```

Expected: fails because `AdapterCapabilities` does not exist.

- [ ] **Step 4: Implement port traits and shared capabilities**

Each trait must use Idiolect-owned types from `idiolect-common` and `idiolect-core` only. Use these minimal signatures for the first contract pass:

```rust
pub struct AdapterCapabilities {
    pub name: String,
    pub version: String,
    pub supports_streaming: bool,
    pub supports_word_timestamps: bool,
    pub supports_confidence: bool,
    pub supports_gpu: bool,
    pub supports_incremental_updates: bool,
}
```

```rust
pub trait AudioInputPort {
    type Error;
    fn start_capture(&mut self, session_id: ImeSessionId) -> Result<(), Self::Error>;
    fn stop_capture(&mut self, session_id: ImeSessionId) -> Result<AudioSegment, Self::Error>;
}

pub trait VadPort {
    type Error;
    fn segment(&mut self, audio: &AudioSegment) -> Result<Vec<AudioSegment>, Self::Error>;
}

pub trait AsrPort {
    type Error;
    fn capabilities(&self) -> AdapterCapabilities;
    fn transcribe(&self, audio: &AudioSegment) -> Result<TranscriptDraft, Self::Error>;
}

pub trait AudioCodecPort {
    type Error;
    fn encode(&self, audio: &AudioSegment) -> Result<EncodedAudio, Self::Error>;
    fn decode(&self, encoded: &EncodedAudio) -> Result<AudioSegment, Self::Error>;
}

pub trait MetadataStorePort {
    type Error;
    fn create_session(&mut self, raw_stt_text: Option<&str>) -> Result<ImeSessionId, Self::Error>;
    fn record_preedit_change(
        &mut self,
        session_id: ImeSessionId,
        from_text: &str,
        to_text: &str,
        event_index: u32,
    ) -> Result<(), Self::Error>;
    fn commit_session(
        &mut self,
        session_id: ImeSessionId,
        committed_text: &str,
        idempotency_key: &str,
    ) -> Result<(), Self::Error>;
    fn cancel_session(
        &mut self,
        session_id: ImeSessionId,
        idempotency_key: &str,
    ) -> Result<(), Self::Error>;
}

pub trait TrainerPort {
    type Error;
    fn train(&self, manifest: TrainingManifest) -> Result<TrainingArtifact, Self::Error>;
}

pub trait EvaluationPort {
    type Error;
    fn evaluate(&self, artifact: TrainingArtifact) -> Result<EvaluationReport, Self::Error>;
}

pub trait AdapterRegistryPort {
    type Error;
    fn register_candidate(&mut self, artifact: TrainingArtifact, report: EvaluationReport) -> Result<String, Self::Error>;
    fn promote(&mut self, adapter_id: &str) -> Result<(), Self::Error>;
    fn rollback(&mut self, user_id: &str) -> Result<(), Self::Error>;
}
```

If a worker needs to change a signature, they must stop and return `NEEDS_CONTEXT`; they do not invent a new API.

- [ ] **Step 5: Verify no backend leakage**

Run:

```bash
rg -n "fcitx|whisper|silero|opus|rusqlite|python|pytorch|peft|burn|candle|onnx" crates/idiolect-ports
```

Expected: no output.

- [ ] **Step 6: Verify**

```bash
cargo test -p idiolect-ports
cargo test -p idiolect-test-support
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/idiolect-ports crates/idiolect-test-support
git commit -m "feat: add port traits and fake contract harness"
```


## Child Plan Acceptance

This child plan is complete only when:

- [ ] The master plan no longer has required-path Python contradictions.
- [ ] The workspace exists and uses pinned toolchain/lints.
- [ ] `idiolect-common`, `idiolect-core`, `idiolect-ports`, and `idiolect-test-support` compile.
- [ ] The first fake port contract test passes.
- [ ] All global Rust gates pass with zero warnings.
- [ ] No backend dependency appears in core, ports, or test-support public APIs.
