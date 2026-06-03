# Idiolect Rust-First Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Idiolect as a Rust-first, local-first Linux speech-to-text input method with strict architecture boundaries, test-first implementation, and lint-clean compilable increments.

**Architecture:** Idiolect uses a ports-and-adapters architecture. `idiolect-core` owns pure product rules, `idiolect-ports` owns replaceable boundaries, `idiolect-application` owns use-case orchestration, adapters own third-party integration, and `idiolectd` is the composition root. Python is research-only and is not part of the v1 runtime, required CI, trainer, promotion path, or packaging path.

**Tech Stack:** Rust workspace, Cargo workspace lints, Tokio, Serde, SQLite via a Rust adapter, Unix domain socket JSON Lines IPC, C++ Fcitx5 shim behind protocol boundaries, fixture-first audio/ASR/VAD adapters, later real CPAL/Silero/whisper-rs/Opus adapters after contracts are green.

---

## Gatekeeper Rules

This project is implemented by bounded sub-agents, but acceptance stays with the gatekeeper. A worker report is evidence, not truth. The gatekeeper must inspect diffs, run commands, and reject work that fails any requirement below.

Hard rules:

- No implementation starts until this plan is approved.
- No production Rust code is accepted without a failing test observed first.
- No lint warning is accepted. Warnings are errors.
- No `#[allow(...)]`, disabled lint, skipped test, ignored test, or warning suppression is accepted unless the plan is amended and the gatekeeper approves the waiver before the code is written.
- No Python dependency is accepted in the v1 runtime path.
- No third-party backend type may appear in any public API of `idiolect-core`, `idiolect-ports`, or `idiolect-application`.
- No sub-agent may change files outside its assigned ownership without asking first.
- Work may proceed on `main` for this implementation because the user explicitly approved it on 2026-06-04. Agents must still keep changes task-scoped and inspect `git status` before edits.
- Rust is pinned to stable `1.96.0`, verified from the official Rust blog release dated 2026-05-28. Rust has no separate LTS channel for this project.

Default Rust gates after every code task:

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

C++ gates once the Fcitx5 shim exists:

```bash
cmake -S fcitx5/idiolect-fcitx5 -B fcitx5/idiolect-fcitx5/build -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_CXX_FLAGS="-Wall -Wextra -Wpedantic -Werror"
cmake --build fcitx5/idiolect-fcitx5/build
ctest --test-dir fcitx5/idiolect-fcitx5/build --output-on-failure
```

Worker evidence required for every task:

```text
Status: DONE, DONE_WITH_CONCERNS, NEEDS_CONTEXT, or BLOCKED
Model used: exact model name
Files changed: exact paths
Red test command: exact command and expected failing assertion
Green test command: exact command and passing result
Lint commands: exact commands and passing result
Notes: any uncertainty, rejected shortcut, or follow-up risk
```

Automatic rejection criteria:

- Any command above fails.
- Any warning appears in compile, lint, or test output.
- A worker writes implementation before a failing test.
- A worker changes architecture, privacy policy, protocol semantics, or promotion gates without gatekeeper approval.
- A worker says "close enough", "not covered", "skipped", or "manual only" for a required automated gate.
- A worker report omits commands, changed files, or failing-test evidence.

## Agent Model Policy

Use `gpt-5.3-codex-spark` for narrow mechanical work with clear file ownership:

- domain value objects
- serde round trips
- pure state machines
- test fixtures
- manifest splitting
- classifier rules
- SQL invariant tests
- small CLI command units

Use a stronger model or gatekeeper-local implementation for judgment-heavy work:

- crate topology changes
- IPC protocol semantics
- Fcitx5 preedit behavior
- privacy/delete semantics
- storage event-log design
- adapter promotion and rollback policy
- real ASR/VAD/training adapters
- packaging and install behavior
- release gate decisions

Sub-agents may review, but gatekeeper acceptance is not delegated.

## Worker Prompt Template

Every implementation sub-agent receives this prompt shape. The gatekeeper fills in only the task-specific file list and test names.

```text
You are a bounded implementation worker for Idiolect. Use model: <model>.

Owned files:
- <exact paths>

Do not edit files outside this ownership list. You are not alone in the codebase; do not revert or overwrite changes made by others.

Required workflow:
1. Write the failing test first.
2. Run the exact red command and confirm the expected failure.
3. Implement the smallest code needed to pass.
4. Run the green command.
5. Run all task-specific gates.
6. Run the global Rust gates unless the task is docs-only.
7. Self-review for architecture leakage, warning suppressions, skipped tests, and accidental scope creep.

Hard constraints:
- No warning suppressions.
- No skipped or ignored tests.
- No Python in v1 required paths.
- No third-party backend type in core, ports, or application public APIs.
- No architecture, protocol, privacy, or promotion-policy decisions.

Return exactly:
Status:
Model used:
Files changed:
Red command and result:
Green command and result:
Lint/check/test commands and results:
Concerns:
```

If a worker cannot satisfy this template, the task is rejected or re-scoped before implementation continues.

## Plan Decomposition

This file is the parent execution plan. It defines architecture, gates, ownership, and milestone order. Large implementation areas must be executed from child plans so each worker receives a small enough brief to complete, compile, test, and review.

Child plans:

```text
docs/superpowers/plans/2026-06-04-idiolect-00-readiness-workspace-core.md
docs/superpowers/plans/2026-06-04-idiolect-01-fake-dictation-loop.md
docs/superpowers/plans/2026-06-04-idiolect-02-storage-event-log.md
docs/superpowers/plans/2026-06-04-idiolect-03-classifier-manifest-promotion.md
docs/superpowers/plans/2026-06-04-idiolect-04-fixture-audio-asr-codec.md
docs/superpowers/plans/2026-06-04-idiolect-05-real-adapters.md
docs/superpowers/plans/2026-06-04-idiolect-06-fcitx5-cli-packaging.md
```

Execution rule: create or update the relevant child plan before starting that milestone. The child plan must contain the exact files, tests, commands, and commit boundaries for that milestone.

## Implementation Decisions Locked By This Plan

These decisions intentionally correct contradictions in `docs/idiolect_master_plan_rust_first.md`.

1. Python is research-only.
   Required v1 paths do not include `python-peft`, `python_trainer`, `pytest`, or Python scripts.

2. Contract tests are Rust-first.
   Required contract tests live in Rust crates and run through Cargo. Python reference checks may exist under `research/`, but they are not release gates.

3. The crate topology is single and explicit.
   The workspace uses `common`, `core`, `ports`, `application`, adapter crates, daemon, CLI, test support, and integration-test crates. Legacy collapsed crate names from the master plan are not used as authoritative architecture.

4. Event log is v1 storage architecture.
   SQLite uses append-only `event_log` plus materialized tables from the first storage implementation to avoid retrofitting audit semantics.

5. Adapter promotion requires artifact compatibility.
   A candidate adapter cannot be promoted unless artifact metadata proves base-model compatibility, manifest digest, metric report digest, and runtime compatibility.

6. Dependency versions are pinned.
   No `*` dependency versions are accepted.

7. Current checkout execution is approved.
   The user explicitly approved working on `main` for this implementation on 2026-06-04.

8. Rust stable 1.96.0 is the toolchain pin.
   If local `rustup` does not have `1.96.0`, install it before workspace gates.

## File Structure

Create this structure over the implementation:

```text
Cargo.toml
rust-toolchain.toml
.cargo/config.toml
README.md
docs/
  superpowers/
    plans/
      2026-06-04-idiolect-v1-rust-first-implementation.md
      2026-06-04-idiolect-00-readiness-workspace-core.md
      2026-06-04-idiolect-01-fake-dictation-loop.md
      2026-06-04-idiolect-02-storage-event-log.md
      2026-06-04-idiolect-03-classifier-manifest-promotion.md
      2026-06-04-idiolect-04-fixture-audio-asr-codec.md
      2026-06-04-idiolect-05-real-adapters.md
      2026-06-04-idiolect-06-fcitx5-cli-packaging.md
  decisions/
    0001-rust-first-v1-architecture.md
  implementation/
    v1-readiness-amendments.md
crates/
  idiolect-common/
    Cargo.toml
    src/lib.rs
    src/ids.rs
    src/time.rs
    src/config.rs
    src/error.rs
    src/protocol.rs
  idiolect-core/
    Cargo.toml
    src/lib.rs
    src/domain/session.rs
    src/domain/candidate.rs
    src/domain/adapter.rs
    src/domain/events.rs
    src/rules/session_lifecycle.rs
    src/rules/promotion.rs
  idiolect-ports/
    Cargo.toml
    src/lib.rs
    src/input_method.rs
    src/audio.rs
    src/vad.rs
    src/asr.rs
    src/codec.rs
    src/storage.rs
    src/trainer.rs
    src/evaluator.rs
    src/adapter_registry.rs
    src/clock.rs
  idiolect-application/
    Cargo.toml
    src/lib.rs
    src/use_cases/dictation.rs
    src/use_cases/correction.rs
    src/use_cases/training.rs
    src/use_cases/promotion.rs
  idiolect-ipc/
    Cargo.toml
    src/lib.rs
    src/messages.rs
    src/framing.rs
    src/handshake.rs
  idiolect-adapter-memory/
    Cargo.toml
    src/lib.rs
  idiolect-adapter-sqlite/
    Cargo.toml
    src/lib.rs
    src/migrations.rs
    src/repository.rs
    migrations/0001_initial.sql
    migrations/0002_correction_memory.sql
  idiolect-adapter-fixture-audio/
    Cargo.toml
    src/lib.rs
  idiolect-adapter-fixture-asr/
    Cargo.toml
    src/lib.rs
  idiolect-adapter-fixture-codec/
    Cargo.toml
    src/lib.rs
  idiolect-trainerctl/
    Cargo.toml
    src/lib.rs
    src/classifier.rs
    src/manifest.rs
    src/metrics.rs
    src/promotion.rs
  idiolectd/
    Cargo.toml
    src/main.rs
    src/lib.rs
    src/configuration.rs
    src/daemon.rs
  idiolect-cli/
    Cargo.toml
    src/main.rs
    src/lib.rs
  idiolect-test-support/
    Cargo.toml
    src/lib.rs
    src/fakes.rs
    src/fixtures.rs
  idiolect-integration-tests/
    Cargo.toml
    tests/fake_dictation_loop.rs
    tests/storage_lifecycle.rs
    tests/privacy_delete.rs
fcitx5/
  idiolect-fcitx5/
    CMakeLists.txt
    src/engine.cpp
    src/engine.h
    src/ipc_client.cpp
    src/ipc_client.h
    tests/preedit_session_test.cpp
ci/
  scripts/test-rust.sh
  scripts/test-integration.sh
  scripts/test-fcitx5.sh
  scripts/test-packaging.sh
research/
  README.md
```

Dependency direction:

```text
idiolect-common -> external utility crates only
idiolect-core -> idiolect-common
idiolect-ports -> idiolect-common, idiolect-core
idiolect-application -> idiolect-common, idiolect-core, idiolect-ports
adapters -> idiolect-common, idiolect-core domain types through ports, idiolect-ports, third-party libraries
idiolect-ipc -> idiolect-common
idiolectd -> idiolect-application, idiolect-ports, idiolect-ipc, selected adapters
idiolect-cli -> idiolect-common, idiolect-application, selected read/query ports
idiolect-test-support -> workspace crates required for fakes and fixtures
idiolect-integration-tests -> workspace crates under test
```

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

## Task 6: Application Use Cases With Fake Ports

**Owner:** Spark worker allowed, gatekeeper reviews orchestration semantics  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-application/src/lib.rs`
- Create: `crates/idiolect-application/src/use_cases/dictation.rs`
- Create: `crates/idiolect-application/src/use_cases/correction.rs`

- [ ] **Step 1: Write failing dictation workflow test**

In `dictation.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::DictationUseCase;
    use idiolect_test_support::fakes::{FakeInputMethod, FakeMetadataStore};

    #[test]
    fn transcript_ready_shows_preedit_and_records_session() {
        let input = FakeInputMethod::default();
        let store = FakeMetadataStore::default();
        let mut use_case = DictationUseCase::new(input, store);

        let session_id = use_case.start_dictation().expect("start should succeed");
        use_case.transcript_ready(session_id, "restart traffic").expect("transcript should apply");

        let input = use_case.input_method();
        assert_eq!(input.events(), ["show_preedit:restart traffic"]);

        let store = use_case.metadata_store();
        assert!(store.session_exists(session_id));
    }
}
```

Run:

```bash
cargo test -p idiolect-application transcript_ready_shows_preedit_and_records_session
```

Expected: fails because `DictationUseCase` and `FakeMetadataStore` do not exist.

- [ ] **Step 2: Implement minimal use case and metadata fake**

Implement the use case as generic over `InputMethodPort` and `MetadataStorePort`. Store a session on start and show preedit when transcript text arrives.

- [ ] **Step 3: Write failing idempotent commit test**

Add:

```rust
#[test]
fn duplicate_commit_creates_one_candidate() {
    let input = FakeInputMethod::default();
    let store = FakeMetadataStore::default();
    let mut use_case = DictationUseCase::new(input, store);

    let session_id = use_case.start_dictation().expect("start should succeed");
    use_case.transcript_ready(session_id, "restart traffic").expect("transcript should apply");
    use_case.commit(session_id, "restart Traefik").expect("first commit should succeed");
    use_case.commit(session_id, "restart Traefik").expect("duplicate commit should be idempotent");

    assert_eq!(use_case.metadata_store().candidate_count(session_id), 1);
}
```

Run:

```bash
cargo test -p idiolect-application duplicate_commit_creates_one_candidate
```

Expected: fails until commit idempotency is implemented.

- [ ] **Step 4: Implement idempotent commit**

Commit must:

```text
1. Return success when the same session/text commit repeats.
2. Create exactly one candidate.
3. Reject cancel after commit.
4. Never call storage twice for duplicate candidate creation.
```

- [ ] **Step 5: Verify**

```bash
cargo test -p idiolect-application
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-application crates/idiolect-test-support
git commit -m "feat: add dictation use case with fake ports"
```

## Task 7: IPC Protocol, JSON Lines Framing, And Handshake

**Owner:** Gatekeeper or stronger model for protocol semantics; Spark may implement parser after tests are fixed  
**Model:** `gpt-5.4` for design, `gpt-5.3-codex-spark` for bounded parser changes  
**Files:**

- Modify: `crates/idiolect-ipc/src/lib.rs`
- Create: `crates/idiolect-ipc/src/messages.rs`
- Create: `crates/idiolect-ipc/src/framing.rs`
- Create: `crates/idiolect-ipc/src/handshake.rs`

- [ ] **Step 1: Write failing JSON Lines framing tests**

In `framing.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{decode_frame, encode_frame};
    use crate::messages::IpcMessage;
    use idiolect_common::ids::ImeSessionId;

    #[test]
    fn frame_round_trip_preserves_message_type() {
        let message = IpcMessage::StartDictation {
            session_id: ImeSessionId::new(),
            user_id: "default".to_owned(),
        };

        let encoded = encode_frame(&message).expect("frame should encode");

        assert!(encoded.ends_with('\n'));
        let decoded = decode_frame(&encoded).expect("frame should decode");
        assert_eq!(decoded.message_type(), "StartDictation");
    }

    #[test]
    fn frame_decoder_rejects_missing_newline() {
        let result = decode_frame(r#"{"type":"StartDictation"}"#);

        assert!(result.is_err());
    }
}
```

Run:

```bash
cargo test -p idiolect-ipc --lib
```

Expected: fails because framing does not exist.

- [ ] **Step 2: Implement message enum and framing**

Define message variants:

```rust
StartDictation { session_id, user_id }
StopDictation { session_id }
TranscriptReady { session_id, utterance_id, text }
ImePreeditChanged { session_id, from, to, event_index }
ImeCommit { session_id, committed_text, command_id }
ImeCancel { session_id, command_id }
Error { code, message }
```

Use adjacently tagged serde:

```rust
#[serde(tag = "type")]
```

- [ ] **Step 3: Write failing handshake tests**

In `handshake.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::negotiate_features;

    #[test]
    fn handshake_accepts_shared_protocol_version() {
        let accepted = negotiate_features(1, &["preedit", "commit", "cancel"])
            .expect("version 1 should be accepted");

        assert_eq!(accepted, ["preedit", "commit", "cancel"]);
    }

    #[test]
    fn handshake_rejects_unknown_protocol_version() {
        let result = negotiate_features(99, &["preedit"]);

        assert!(result.is_err());
    }
}
```

Run:

```bash
cargo test -p idiolect-ipc --lib
```

Expected: fails because handshake logic does not exist.

- [ ] **Step 4: Implement handshake**

Protocol version `1` is accepted. Other versions return a structured protocol error.

- [ ] **Step 5: Verify**

```bash
cargo test -p idiolect-ipc
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-ipc
git commit -m "feat: add versioned ipc jsonl protocol"
```

## Task 8: Daemon Composition With Fixture Adapters And E2E-Lite Test

**Owner:** Gatekeeper for integration; Spark may implement isolated fixture adapters  
**Model:** `gpt-5.4` or gatekeeper-local  
**Files:**

- Modify: `crates/idiolectd/src/lib.rs`
- Create: `crates/idiolectd/src/daemon.rs`
- Modify: `crates/idiolect-integration-tests/Cargo.toml`
- Create: `crates/idiolect-integration-tests/tests/fake_dictation_loop.rs`

- [ ] **Step 1: Write failing E2E-lite test**

Create `fake_dictation_loop.rs`:

```rust
use idiolectd::daemon::FixtureDaemon;

#[test]
fn fake_dictation_loop_corrects_and_commits_one_session() {
    let mut daemon = FixtureDaemon::new();

    let session_id = daemon.start_dictation().expect("dictation should start");
    daemon.inject_transcript(session_id, "restart traffic").expect("transcript should inject");
    daemon.preedit_changed(session_id, "restart traffic", "restart Traefik", 1)
        .expect("preedit change should record");
    daemon.commit(session_id, "restart Traefik").expect("commit should succeed");

    let report = daemon.session_report(session_id).expect("session report should exist");
    assert_eq!(report.raw_stt_text, "restart traffic");
    assert_eq!(report.committed_text, "restart Traefik");
    assert_eq!(report.training_candidate_count, 1);
}
```

Run:

```bash
cargo test -p idiolect-integration-tests fake_dictation_loop_corrects_and_commits_one_session
```

Expected: fails because `FixtureDaemon` does not exist.

- [ ] **Step 2: Implement fixture daemon**

`FixtureDaemon` wires:

```text
DictationUseCase
FakeInputMethod
FakeMetadataStore
Fixture ASR transcript injection
```

It must not open a real microphone, socket, model, database, or desktop integration.

- [ ] **Step 3: Add duplicate and cancel scenarios**

Add tests:

```rust
#[test]
fn duplicate_commit_does_not_duplicate_candidate() {
    let mut daemon = FixtureDaemon::new();
    let session_id = daemon.start_dictation().expect("dictation should start");
    daemon.inject_transcript(session_id, "deploy container").expect("transcript should inject");
    daemon.commit(session_id, "deploy the container").expect("commit should succeed");
    daemon.commit(session_id, "deploy the container").expect("duplicate commit should succeed");

    let report = daemon.session_report(session_id).expect("session report should exist");
    assert_eq!(report.training_candidate_count, 1);
}

#[test]
fn cancel_does_not_commit_or_create_candidate() {
    let mut daemon = FixtureDaemon::new();
    let session_id = daemon.start_dictation().expect("dictation should start");
    daemon.inject_transcript(session_id, "open vault warden").expect("transcript should inject");
    daemon.cancel(session_id).expect("cancel should succeed");

    let report = daemon.session_report(session_id).expect("session report should exist");
    assert_eq!(report.committed_text, "");
    assert_eq!(report.training_candidate_count, 0);
}
```

Run:

```bash
cargo test -p idiolect-integration-tests
```

Expected: fails until duplicate and cancel behavior is implemented.

- [ ] **Step 4: Implement behavior**

Implement only fixture-driven daemon behavior needed by the tests.

- [ ] **Step 5: Verify**

```bash
cargo test -p idiolectd
cargo test -p idiolect-integration-tests
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolectd crates/idiolect-integration-tests
git commit -m "feat: add fixture daemon e2e-lite loop"
```

## Task 9: SQLite Event Log And Materialized Tables

**Owner:** Spark worker for SQL and repository tests, gatekeeper reviews invariants  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-adapter-sqlite/src/lib.rs`
- Create: `crates/idiolect-adapter-sqlite/src/migrations.rs`
- Create: `crates/idiolect-adapter-sqlite/src/repository.rs`
- Create: `crates/idiolect-adapter-sqlite/migrations/0001_initial.sql`
- Create: `crates/idiolect-integration-tests/tests/storage_lifecycle.rs`

- [ ] **Step 1: Write failing migration test**

In `repository.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::SqliteMetadataStore;

    #[test]
    fn migration_creates_event_log_and_materialized_tables() {
        let store = SqliteMetadataStore::in_memory().expect("sqlite should open");
        store.migrate().expect("migration should apply");

        let tables = store.table_names().expect("table names should load");

        assert!(tables.contains(&"event_log".to_owned()));
        assert!(tables.contains(&"ime_text_sessions".to_owned()));
        assert!(tables.contains(&"training_candidates".to_owned()));
    }
}
```

Run:

```bash
cargo test -p idiolect-adapter-sqlite migration_creates_event_log_and_materialized_tables
```

Expected: fails because SQLite store does not exist.

- [ ] **Step 2: Implement migration**

`0001_initial.sql` must create:

```sql
CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  applied_at TEXT NOT NULL,
  checksum TEXT NOT NULL
);

CREATE TABLE event_log (
  id TEXT PRIMARY KEY,
  aggregate_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  event_version INTEGER NOT NULL,
  event_json TEXT NOT NULL,
  idempotency_key TEXT,
  created_at TEXT NOT NULL
);

CREATE UNIQUE INDEX event_log_idempotency_key_unique
ON event_log(idempotency_key)
WHERE idempotency_key IS NOT NULL;
```

Add materialized tables for users, utterances, IME sessions, edit events, training candidates, correction memory entries, adapters, and training runs. After `0001_initial.sql` is committed, it is immutable; later schema changes must use a new numbered migration such as `0002_correction_memory.sql`.

- [ ] **Step 3: Write failing transactional lifecycle test**

In `storage_lifecycle.rs`:

```rust
use idiolect_adapter_sqlite::SqliteMetadataStore;

#[test]
fn committed_session_writes_event_and_candidate_atomically() {
    let store = SqliteMetadataStore::in_memory().expect("sqlite should open");
    store.migrate().expect("migration should apply");

    let session_id = store.create_test_session("restart traffic").expect("session should create");
    store.commit_test_session(session_id, "restart Traefik", "cmd-1")
        .expect("commit should succeed");
    store.commit_test_session(session_id, "restart Traefik", "cmd-1")
        .expect("duplicate commit should be idempotent");

    assert_eq!(store.event_count(session_id).expect("events should count"), 2);
    assert_eq!(store.candidate_count(session_id).expect("candidates should count"), 1);
}
```

Run:

```bash
cargo test -p idiolect-integration-tests committed_session_writes_event_and_candidate_atomically
```

Expected: fails until repository methods and idempotency are implemented.

- [ ] **Step 4: Implement repository behavior**

Implement:

```text
append event first
update materialized tables in the same transaction
deduplicate idempotency keys
return success for duplicate commit with same idempotency key
return a structured conflict for same idempotency key with different payload
```

- [ ] **Step 5: Verify**

```bash
cargo test -p idiolect-adapter-sqlite
cargo test -p idiolect-integration-tests storage_lifecycle
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-adapter-sqlite crates/idiolect-integration-tests/tests/storage_lifecycle.rs
git commit -m "feat: add sqlite event log storage"
```

## Task 10: Offline Classifier And Correction Memory

**Owner:** Spark worker allowed, gatekeeper reviews training-data safety  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-trainerctl/src/lib.rs`
- Create: `crates/idiolect-trainerctl/src/classifier.rs`
- Create: `crates/idiolect-trainerctl/src/manifest.rs`
- [ ] **Step 1: Write failing classifier fixture tests**

In `classifier.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{classify_edit, EditClassification};

    #[test]
    fn proper_noun_correction_is_high_trust() {
        let result = classify_edit("restart traffic", "restart Traefik");

        assert_eq!(result.label, EditClassification::ProperNounCorrection);
        assert_eq!(result.trust_score, 0.9);
        assert!(result.training_allowed);
    }

    #[test]
    fn semantic_rewrite_is_rejected() {
        let result = classify_edit("restart traffic", "actually open the notes");

        assert_eq!(result.label, EditClassification::SemanticRewrite);
        assert_eq!(result.trust_score, 0.0);
        assert!(!result.training_allowed);
    }

    #[test]
    fn accepted_without_edit_is_weak_training_signal() {
        let result = classify_edit("deploy the container", "deploy the container");

        assert_eq!(result.label, EditClassification::AcceptedWithoutEdit);
        assert_eq!(result.trust_score, 0.6);
        assert!(result.training_allowed);
    }
}
```

Run:

```bash
cargo test -p idiolect-trainerctl --lib
```

Expected: fails because classifier does not exist.

- [ ] **Step 2: Implement deterministic classifier rules**

Implement exact mappings for the fixture cases. Use conservative rejection when a rule cannot classify safely.

- [ ] **Step 3: Write failing manifest exclusion test**

In `manifest.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{ManifestBuilder, ManifestCandidate};

    #[test]
    fn rejected_candidates_are_excluded_from_manifest() {
        let candidates = vec![
            ManifestCandidate::approved_for_test("u1", "restart Traefik", 0.9),
            ManifestCandidate::rejected_for_test("u2", "actually open the notes"),
        ];

        let manifest = ManifestBuilder::new().build(candidates).expect("manifest should build");

        assert_eq!(manifest.entries().len(), 1);
        assert_eq!(manifest.entries()[0].utterance_id, "u1");
    }
}
```

Run:

```bash
cargo test -p idiolect-trainerctl rejected_candidates_are_excluded_from_manifest
```

Expected: fails because manifest builder does not exist.

- [ ] **Step 4: Implement manifest builder**

The builder must:

```text
exclude rejected candidates
exclude semantic rewrites
keep holdout entries out of train split
include utterance id, audio path, transcript, source, trust score, split
produce deterministic ordering by utterance id
```

- [ ] **Step 5: Verify**

```bash
cargo test -p idiolect-trainerctl
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-trainerctl
git commit -m "feat: add classifier and manifest safety rules"
```

## Task 11: Evaluation, Artifact Compatibility, Promotion, And Rollback

**Owner:** Gatekeeper for policy; Spark may implement matrix tests after policy is written  
**Model:** `gpt-5.4` for policy, `gpt-5.3-codex-spark` for pure matrix code  
**Files:**

- Create: `crates/idiolect-trainerctl/src/metrics.rs`
- Create: `crates/idiolect-trainerctl/src/promotion.rs`
- Create: `crates/idiolect-core/src/domain/adapter.rs`
- Create: `crates/idiolect-core/src/rules/promotion.rs`

- [ ] **Step 1: Write failing promotion matrix tests**

In `promotion.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{evaluate_promotion, ArtifactCompatibility, EvaluationReport, PromotionDecision};

    #[test]
    fn clear_improvement_with_compatible_artifact_promotes() {
        let report = EvaluationReport::for_test()
            .personal_wer_delta(-0.05)
            .general_wer_delta(0.0)
            .proper_noun_accuracy_delta(0.02)
            .hallucination_delta(0.0)
            .p95_latency_delta_ms(0);

        let compatibility = ArtifactCompatibility::compatible_for_test();

        assert_eq!(evaluate_promotion(report, compatibility), PromotionDecision::Promote);
    }

    #[test]
    fn incompatible_artifact_rejects_even_when_metrics_improve() {
        let report = EvaluationReport::for_test().personal_wer_delta(-0.10);
        let compatibility = ArtifactCompatibility::base_model_mismatch_for_test();

        assert_eq!(evaluate_promotion(report, compatibility), PromotionDecision::Reject);
    }

    #[test]
    fn hallucination_regression_rejects() {
        let report = EvaluationReport::for_test()
            .personal_wer_delta(-0.05)
            .hallucination_delta(0.01);
        let compatibility = ArtifactCompatibility::compatible_for_test();

        assert_eq!(evaluate_promotion(report, compatibility), PromotionDecision::Reject);
    }
}
```

Run:

```bash
cargo test -p idiolect-trainerctl --lib
```

Expected: fails because promotion policy does not exist.

- [ ] **Step 2: Implement promotion policy**

Promotion requires:

```text
artifact base_model equals active base_model
artifact manifest_digest exists
artifact metrics_digest exists
artifact runtime_compatibility equals compatible
personal WER improves
general WER does not regress
proper noun accuracy does not regress
hallucination rate does not increase
p95 latency does not exceed the configured threshold
```

- [ ] **Step 3: Write failing rollback test**

Add:

```rust
#[test]
fn rollback_restores_previous_active_adapter() {
    let registry = super::InMemoryAdapterRegistry::for_test()
        .with_active("adapter-v2")
        .with_previous("adapter-v1");

    let registry = registry.rollback().expect("rollback should succeed");

    assert_eq!(registry.active_adapter_id(), "adapter-v1");
    assert_eq!(registry.adapter_status("adapter-v2"), "rolled_back");
}
```

Run:

```bash
cargo test -p idiolect-trainerctl rollback_restores_previous_active_adapter
```

Expected: fails until registry behavior exists.

- [ ] **Step 4: Implement rollback**

Rollback must be atomic in storage-backed implementations and deterministic in the in-memory test registry.

- [ ] **Step 5: Verify**

```bash
cargo test -p idiolect-core
cargo test -p idiolect-trainerctl
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-core crates/idiolect-trainerctl
git commit -m "feat: add adapter promotion and rollback gates"
```

## Task 12: Fixture Audio, Codec, VAD, And ASR Adapters

**Owner:** Spark worker for fixture adapters only  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-adapter-fixture-audio/src/lib.rs`
- Modify: `crates/idiolect-adapter-fixture-asr/src/lib.rs`
- Modify: `crates/idiolect-adapter-fixture-codec/src/lib.rs`
- Create: `crates/idiolect-test-support/src/fixtures.rs`

- [ ] **Step 1: Write failing audio fixture test**

In `fixtures.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::sine_fixture_16khz_mono;

    #[test]
    fn sine_fixture_has_expected_sample_rate_and_duration() {
        let fixture = sine_fixture_16khz_mono();

        assert_eq!(fixture.sample_rate_hz, 16_000);
        assert_eq!(fixture.channels, 1);
        assert_eq!(fixture.duration_ms, 1_000);
        assert_eq!(fixture.samples.len(), 16_000);
    }
}
```

Run:

```bash
cargo test -p idiolect-test-support sine_fixture_has_expected_sample_rate_and_duration
```

Expected: fails because fixture does not exist.

- [ ] **Step 2: Implement deterministic audio fixture**

Implement a generated one-second mono sine buffer. Do not commit binary audio.

- [ ] **Step 3: Write failing fixture ASR test**

In `idiolect-adapter-fixture-asr/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::FixtureAsr;
    use idiolect_ports::asr::AsrPort;
    use idiolect_test_support::fixtures::sine_fixture_16khz_mono;

    #[test]
    fn fixture_asr_returns_configured_transcript() {
        let asr = FixtureAsr::new("restart traffic");
        let draft = asr.transcribe(&sine_fixture_16khz_mono()).expect("transcription should succeed");

        assert_eq!(draft.text, "restart traffic");
        assert_eq!(draft.metadata.engine_name, "fixture-asr");
    }
}
```

Run:

```bash
cargo test -p idiolect-adapter-fixture-asr fixture_asr_returns_configured_transcript
```

Expected: fails because fixture ASR does not exist.

- [ ] **Step 4: Implement fixture ASR and codec adapters**

Fixture codec must round-trip in memory without third-party codec libraries. It proves the `AudioCodecPort` contract before real Opus integration.

- [ ] **Step 5: Verify**

```bash
cargo test -p idiolect-adapter-fixture-audio
cargo test -p idiolect-adapter-fixture-asr
cargo test -p idiolect-adapter-fixture-codec
cargo test -p idiolect-test-support
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-adapter-fixture-audio crates/idiolect-adapter-fixture-asr crates/idiolect-adapter-fixture-codec crates/idiolect-test-support
git commit -m "feat: add fixture audio asr and codec adapters"
```

## Task 13: CPAL Audio Adapter Gate

**Owner:** Gatekeeper for dependency acceptance; Spark only after exact dependency decision is recorded  
**Model:** Gatekeeper-local or `gpt-5.4` for dependency decision, `gpt-5.3-codex-spark` for bounded implementation  
**Files:**

- Create after dependency review: `crates/idiolect-adapter-cpal/`
- Modify after contract test exists: `Cargo.toml`
- Create: `docs/decisions/0002-cpal-audio-adapter.md`

- [ ] **Step 1: Record dependency decision**

The decision record must include crate name, exact version, license, native system dependencies, feature flags, build risk, hidden third-party types, and the `AudioInputPort` contract test that exercises the adapter.

- [ ] **Step 2: Write failing `AudioInputPort` contract test**

The test must prove the adapter reports capabilities, rejects unavailable devices with an Idiolect-owned error, and does not expose CPAL types in public APIs.

- [ ] **Step 3: Implement the adapter behind the port**

Add the dependency only after the contract test exists. Use an exact dependency version and no broad feature set without a decision-record reason.

- [ ] **Step 4: Verify**

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/idiolect-adapter-cpal docs/decisions/0002-cpal-audio-adapter.md
git commit -m "feat: add cpal audio adapter behind contract"
```

## Task 14: Opus Codec Adapter Gate

**Owner:** Gatekeeper for dependency acceptance; Spark allowed for bounded implementation  
**Model:** `gpt-5.3-codex-spark` after dependency decision  
**Files:**

- Create after dependency review: `crates/idiolect-adapter-opus/`
- Modify after contract test exists: `Cargo.toml`
- Create: `docs/decisions/0003-opus-codec-adapter.md`

- [ ] **Step 1: Record dependency decision**

The decision record must include exact `opus`/`ogg` crate versions or a deliberate alternative, native system dependencies, license, and round-trip tolerance.

- [ ] **Step 2: Write failing `AudioCodecPort` contract test**

The test must prove encode/decode round-trip on the deterministic sine fixture, stable hash metadata, and structured failure for corrupt encoded data.

- [ ] **Step 3: Implement the adapter behind the port**

No third-party codec type may escape `idiolect-adapter-opus` public APIs.

- [ ] **Step 4: Verify**

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/idiolect-adapter-opus docs/decisions/0003-opus-codec-adapter.md
git commit -m "feat: add opus codec adapter behind contract"
```

## Task 15: VAD Adapter Gate

**Owner:** Gatekeeper for dependency acceptance; Spark only after exact dependency decision is recorded  
**Model:** Gatekeeper-local or `gpt-5.4` for dependency decision  
**Files:**

- Create after dependency review: `crates/idiolect-adapter-vad/`
- Modify after contract test exists: `Cargo.toml`
- Create: `docs/decisions/0004-vad-adapter.md`

- [ ] **Step 1: Record dependency decision**

The decision record must compare available Rust VAD options and choose one exact dependency/version or a fixture-only deferment. Wildcard versions are rejected.

- [ ] **Step 2: Write failing `VadPort` contract test**

The test must prove silence emits no speech segment, fixture speech emits one bounded segment, pre-roll clamps to zero, and max utterance duration is enforced.

- [ ] **Step 3: Implement the adapter behind the port**

No ONNX, Silero, or third-party model type may escape the adapter crate public API.

- [ ] **Step 4: Verify**

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/idiolect-adapter-vad docs/decisions/0004-vad-adapter.md
git commit -m "feat: add vad adapter behind contract"
```

## Task 16: Whisper ASR Adapter Gate

**Owner:** Gatekeeper-only for first implementation because model/runtime behavior affects product quality and build stability  
**Model:** Gatekeeper-local or `gpt-5.4`  
**Files:**

- Create after dependency review: `crates/idiolect-adapter-whisper-rs/`
- Modify after contract test exists: `Cargo.toml`
- Create: `docs/decisions/0005-whisper-rs-asr-adapter.md`

- [ ] **Step 1: Record dependency decision**

The decision record must include exact `whisper-rs` version, feature flags, native dependencies, model fixture strategy, CPU/GPU build stance, and why the dependency remains behind `AsrPort`.

- [ ] **Step 2: Write failing `AsrPort` contract test**

The test must assert stable metadata, structured missing-model error, empty-audio behavior, and no public whisper-rs type leakage. Real transcription accuracy tests belong in model-regression jobs, not this first contract test.

- [ ] **Step 3: Implement the adapter behind the port**

The first implementation may be CPU-only if that is the smallest reliable contract pass. GPU features require a separate decision and gate.

- [ ] **Step 4: Verify**

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/idiolect-adapter-whisper-rs docs/decisions/0005-whisper-rs-asr-adapter.md
git commit -m "feat: add whisper asr adapter behind contract"
```

## Task 17: Fcitx5 Thin Shim

**Owner:** Gatekeeper-only for first implementation  
**Model:** Gatekeeper-local or `gpt-5.4`  
**Files:**

- Create: `fcitx5/idiolect-fcitx5/CMakeLists.txt`
- Create: `fcitx5/idiolect-fcitx5/src/engine.cpp`
- Create: `fcitx5/idiolect-fcitx5/src/engine.h`
- Create: `fcitx5/idiolect-fcitx5/src/ipc_client.cpp`
- Create: `fcitx5/idiolect-fcitx5/src/ipc_client.h`
- Create: `fcitx5/idiolect-fcitx5/tests/preedit_session_test.cpp`
- Create: `ci/scripts/test-fcitx5.sh`

- [ ] **Step 1: Write failing C++ preedit session test**

Create a test that asserts:

```text
show preedit happens before commit
cancel clears preedit
duplicate commit emits one commit event
IPC disconnect does not write storage or learning data from C++ side
```

- [ ] **Step 2: Implement thin C++ state holder**

The shim may:

```text
track current session id
send StartDictation and StopDictation IPC messages
show, update, commit, and cancel preedit through Fcitx5 APIs
send edit, commit, and cancel events to idiolectd
```

The shim must not:

```text
capture audio
run ASR
write SQLite
write audio files
classify corrections
train models
promote adapters
read clipboard
observe unrelated text
```

- [ ] **Step 3: Add C++ gate script**

Create `ci/scripts/test-fcitx5.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

cmake -S fcitx5/idiolect-fcitx5 -B fcitx5/idiolect-fcitx5/build -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_CXX_FLAGS="-Wall -Wextra -Wpedantic -Werror"
cmake --build fcitx5/idiolect-fcitx5/build
ctest --test-dir fcitx5/idiolect-fcitx5/build --output-on-failure
```

- [ ] **Step 4: Verify**

```bash
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add fcitx5/idiolect-fcitx5 ci/scripts/test-fcitx5.sh
git commit -m "feat: add thin fcitx5 shim"
```

## Task 18: CLI, Privacy Commands, And Doctor Reports

**Owner:** Gatekeeper for privacy semantics; Spark may implement individual command parsing after tests exist  
**Model:** `gpt-5.4` for privacy, `gpt-5.3-codex-spark` for narrow CLI parsing  
**Files:**

- Modify: `crates/idiolect-cli/src/lib.rs`
- Modify: `crates/idiolect-cli/src/main.rs`
- Create: `crates/idiolect-integration-tests/tests/privacy_delete.rs`

- [ ] **Step 1: Write failing privacy delete test**

In `privacy_delete.rs`:

```rust
use idiolectd::daemon::FixtureDaemon;

#[test]
fn deleting_session_removes_audio_text_events_candidates_and_manifest_refs() {
    let mut daemon = FixtureDaemon::new();
    let session_id = daemon.create_committed_fixture_session(
        "restart traffic",
        "restart Traefik",
    );

    daemon.delete_session(session_id).expect("delete should succeed");

    let report = daemon.privacy_report(session_id);
    assert!(!report.audio_exists);
    assert!(!report.text_session_exists);
    assert!(!report.edit_events_exist);
    assert!(!report.candidates_exist);
    assert!(!report.manifest_references_exist);
}
```

Run:

```bash
cargo test -p idiolect-integration-tests deleting_session_removes_audio_text_events_candidates_and_manifest_refs
```

Expected: fails because delete/report behavior does not exist.

- [ ] **Step 2: Implement delete/report behavior in application and storage fakes**

Deletion must remove or tombstone all derived records according to strict privacy mode.

- [ ] **Step 3: Write CLI command tests**

Add tests for:

```text
idiolect doctor
idiolect sessions list
idiolect sessions show <id> rejects private text by default
idiolect sessions show <id> --show-text includes private text
idiolect privacy delete-session <id>
```

- [ ] **Step 4: Implement CLI commands**

Command output must not include transcript text unless `--show-text` or `--include-private` is present.

- [ ] **Step 5: Verify**

```bash
cargo test -p idiolect-cli
cargo test -p idiolect-integration-tests privacy_delete
bash ci/scripts/test-rust.sh
```

Expected: all pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-cli crates/idiolect-integration-tests crates/idiolectd
git commit -m "feat: add cli privacy and doctor commands"
```

## Task 19: Packaging And Release Gate Scripts

**Owner:** Gatekeeper-only until package boundaries are stable  
**Model:** Gatekeeper-local or `gpt-5.4`  
**Files:**

- Create: `ci/scripts/test-integration.sh`
- Create: packaging files after install layout is confirmed by the packaging child plan

- [ ] **Step 1: Write integration script**

Create `ci/scripts/test-integration.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

cargo test -p idiolect-integration-tests --all-targets --all-features
```

- [ ] **Step 2: Defer packaging smoke script until packaging files exist**

Do not create `ci/scripts/test-packaging.sh` in this task. A gate script must not be committed before it can pass. The packaging child plan must create the script in the same task that creates package artifacts, and the first committed version of the script must pass.

- [ ] **Step 3: Add package install tests after Debian package exists**

The package test must prove:

```text
daemon binary installed
CLI binary installed
Fcitx5 addon installed
user service can start and stop
uninstall preserves user data
purge removes user data only with explicit purge action
schema migration runs during upgrade
```

- [ ] **Step 4: Verify all gates**

Run:

```bash
bash ci/scripts/test-rust.sh
bash ci/scripts/test-integration.sh
bash ci/scripts/test-fcitx5.sh
```

Expected: Rust, integration, and Fcitx5 gates pass with zero warnings. Packaging is verified by the packaging child plan once package artifacts exist.

- [ ] **Step 6: Commit**

```bash
git add ci packaging
git commit -m "ci: add integration and packaging release gates"
```

## Final Acceptance Checklist

The gatekeeper may mark a milestone complete only when every item below is true:

- [ ] All changed behavior was test-first.
- [ ] Every crate compiles.
- [ ] `cargo fmt` passes.
- [ ] `cargo clippy` passes with `-D warnings`.
- [ ] `RUSTFLAGS="-D warnings" cargo check` passes.
- [ ] `cargo test --workspace --all-targets --all-features` passes.
- [ ] `cargo test --workspace --doc --all-features` passes.
- [ ] Required contract tests pass for every touched port.
- [ ] Integration tests pass for every touched workflow.
- [ ] C++ shim gates pass when C++ is touched.
- [ ] No warning suppressions were added without prior gatekeeper approval.
- [ ] No Python dependency entered a v1 required path.
- [ ] No third-party backend type leaked into core, ports, or application public APIs.
- [ ] Privacy and delete invariants pass when storage, session, or manifest code changes.
- [ ] Promotion and rollback invariants pass when learning code changes.
- [ ] The worker's report includes exact commands and changed files.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-04-idiolect-v1-rust-first-implementation.md`. Two execution options:

1. **Subagent-Driven (recommended)** - dispatch a fresh subagent per task, review between tasks, and reject anything that is not lint-clean, tested, and architecturally compliant.
2. **Inline Execution** - execute tasks in this session using executing-plans, with checkpoints and gatekeeper review.

Recommended execution path: Subagent-driven, with `gpt-5.3-codex-spark` only for bounded mechanical tasks and stronger/gatekeeper execution for architecture, IPC, privacy, Fcitx5, promotion, and packaging tasks.
