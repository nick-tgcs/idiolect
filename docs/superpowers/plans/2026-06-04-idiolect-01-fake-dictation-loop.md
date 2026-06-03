# Idiolect 01 Fake Dictation Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a deterministic fake dictation loop that exercises application orchestration from transcript to preedit correction to commit or cancel.

**Architecture:** This milestone stays in-process and fake-backed. `idiolect-application` owns use-case sequencing, `idiolect-test-support` owns fakes, and `idiolectd` exposes fixture composition for tests only.

**Tech Stack:** Rust workspace crates from child 00, fake `InputMethodPort`, fake `MetadataStorePort`, strict Cargo lint gates, no real audio, no real storage, no socket IPC, no Fcitx5 shim.

---

## Scope Boundary

Allowed behavior:

```text
application dictation use case
fake metadata store
fake input method event assertions
fixture daemon composition
fake integration tests for commit, duplicate commit, and cancel
```

Forbidden behavior:

```text
SQLite writes
real audio capture
real ASR
real VAD
real codec
Unix socket IPC
Fcitx5 C++ changes
Python required-path code
```

Required gates after every code task:

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

## Task 1: Fake Metadata Store And Dictation Use Case

**Owner:** Spark worker allowed, gatekeeper reviews use-case semantics  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `Cargo.lock`
- Modify: `crates/idiolect-application/Cargo.toml`
- Modify: `crates/idiolect-application/src/lib.rs`
- Create: `crates/idiolect-application/src/use_cases/dictation.rs`
- Modify: `crates/idiolect-test-support/src/lib.rs`
- Modify: `crates/idiolect-test-support/src/fakes.rs`

- [ ] **Step 1: Write failing application tests**

Create these tests in `crates/idiolect-application/src/use_cases/dictation.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::DictationUseCase;
    use idiolect_test_support::fakes::{FakeInputMethod, FakeMetadataStore};

    #[test]
    fn transcript_ready_shows_preedit_and_records_session() {
        let mut use_case = DictationUseCase::new(
            FakeInputMethod::default(),
            FakeMetadataStore::default(),
        );

        let session_id = use_case.start_dictation().expect("session should start");
        use_case.transcript_ready(session_id, "restart traffic").unwrap();

        assert_eq!(use_case.input_events(), ["show_preedit:restart traffic"]);
        assert_eq!(use_case.storage_events(), ["create_session:<none>"]);
    }

    #[test]
    fn correction_then_duplicate_commit_records_one_training_candidate() {
        let mut use_case = DictationUseCase::new(
            FakeInputMethod::default(),
            FakeMetadataStore::default(),
        );

        let session_id = use_case.start_dictation().expect("session should start");
        use_case.transcript_ready(session_id, "restart traffic").unwrap();
        use_case.correct_preedit(session_id, "restart traffic", "restart Traefik", 0).unwrap();
        use_case.commit(session_id, "restart Traefik", "commit-session-1").unwrap();
        use_case.commit(session_id, "restart Traefik", "commit-session-1").unwrap();

        assert_eq!(use_case.input_events(), [
            "show_preedit:restart traffic",
            "update_preedit:restart Traefik",
            "commit:restart Traefik",
        ]);
        assert_eq!(use_case.training_candidate_count(), 1);
    }

    #[test]
    fn cancel_after_preedit_clears_input_and_records_no_candidate() {
        let mut use_case = DictationUseCase::new(
            FakeInputMethod::default(),
            FakeMetadataStore::default(),
        );

        let session_id = use_case.start_dictation().expect("session should start");
        use_case.transcript_ready(session_id, "open notes").unwrap();
        use_case.cancel(session_id, "cancel-session-1").unwrap();

        assert_eq!(use_case.input_events(), ["show_preedit:open notes", "cancel_preedit"]);
        assert_eq!(use_case.training_candidate_count(), 0);
    }
}
```

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-application --lib
```

Expected: FAIL because `DictationUseCase` and `FakeMetadataStore` are absent.

- [ ] **Step 3: Implement fake metadata storage**

Implement `FakeMetadataStore` in `crates/idiolect-test-support/src/fakes.rs` with an event log, an idempotency-key set, and a candidate counter. Its `MetadataStorePort` behavior is:

```text
create_session(None) records create_session:<none> and returns a new ImeSessionId
record_preedit_change records correction:<from>-><to>:<index>
commit_session with a new key records commit:<text> and increments candidate count once
commit_session with the same key records no event and keeps candidate count unchanged
cancel_session with a new key records cancel and does not increment candidate count
```

Expose:

```rust
impl FakeMetadataStore {
    pub fn events(&self) -> Vec<&str>;
    pub fn training_candidate_count(&self) -> usize;
}
```

- [ ] **Step 4: Implement dictation use case**

Implement `DictationUseCase<I, S>` generic over `InputMethodPort` and `MetadataStorePort` with:

```rust
pub fn new(input: I, storage: S) -> Self;
pub fn start_dictation(&mut self) -> Result<ImeSessionId, DictationUseCaseError<I::Error, S::Error>>;
pub fn transcript_ready(&mut self, session_id: ImeSessionId, text: &str) -> Result<(), DictationUseCaseError<I::Error, S::Error>>;
pub fn correct_preedit(&mut self, session_id: ImeSessionId, from_text: &str, to_text: &str, event_index: u32) -> Result<(), DictationUseCaseError<I::Error, S::Error>>;
pub fn commit(&mut self, session_id: ImeSessionId, committed_text: &str, idempotency_key: &str) -> Result<(), DictationUseCaseError<I::Error, S::Error>>;
pub fn cancel(&mut self, session_id: ImeSessionId, idempotency_key: &str) -> Result<(), DictationUseCaseError<I::Error, S::Error>>;
```

The method sequence is fixed: `start_dictation` creates storage session, `transcript_ready` shows preedit, `correct_preedit` records storage change then updates preedit, `commit` calls storage, records successful idempotency keys in the use case, and calls input commit only the first time that key succeeds, and `cancel` calls storage then input cancel.

- [ ] **Step 5: Run green command and gates**

```bash
cargo test -p idiolect-application --lib
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.lock crates/idiolect-application crates/idiolect-test-support docs/superpowers/plans/2026-06-04-idiolect-01-fake-dictation-loop.md
git commit -m "feat: add fake-backed dictation use case"
```

## Task 2: Fixture Daemon Composition

**Owner:** Spark worker allowed, gatekeeper reviews daemon boundary  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `Cargo.lock`
- Modify: `crates/idiolectd/src/lib.rs`
- Create: `crates/idiolectd/src/daemon.rs`
- Modify: `crates/idiolectd/Cargo.toml`

- [ ] **Step 1: Write failing daemon tests**

Create tests in `crates/idiolectd/src/daemon.rs` for `fixture_daemon_commits_corrected_text_once` and `fixture_daemon_cancel_records_no_candidate`. The commit test must call `begin_fake_dictation`, `correct`, `commit`, and duplicate `commit`, then assert the input events are `show_preedit`, `update_preedit`, `commit`, and candidate count is `1`.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolectd --lib
```

Expected: FAIL because `FixtureDaemon` is absent.

- [ ] **Step 3: Implement fixture daemon**

Implement `FixtureDaemon` as a thin wrapper around `DictationUseCase<FakeInputMethod, FakeMetadataStore>`:

```text
new_for_tests(transcript) stores deterministic transcript text
begin_fake_dictation starts a use-case session and immediately calls transcript_ready
correct delegates to correct_preedit
commit delegates to commit
cancel delegates to cancel
input_events and training_candidate_count delegate to use-case accessors
```

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolectd --lib
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock crates/idiolectd docs/superpowers/plans/2026-06-04-idiolect-01-fake-dictation-loop.md
git commit -m "feat: add fixture daemon dictation composition"
```

## Task 3: Fake Dictation Integration Tests

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `Cargo.lock`
- Modify: `crates/idiolectd/src/lib.rs`
- Modify: `crates/idiolect-integration-tests/Cargo.toml`
- Create: `crates/idiolect-integration-tests/tests/fake_dictation_loop.rs`

- [ ] **Step 1: Write failing integration tests**

Create integration tests named:

```text
fake_dictation_loop_corrects_and_commits_one_session
fake_dictation_loop_duplicate_commit_is_idempotent
fake_dictation_loop_cancel_clears_preedit_without_candidate
```

Each test must use `idiolectd::daemon::FixtureDaemon`. The commit test asserts `training_candidate_count() == 1`; the cancel test asserts `training_candidate_count() == 0`.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-integration-tests --test fake_dictation_loop
```

Expected: FAIL until `idiolect-integration-tests` depends on `idiolectd` and `FixtureDaemon` is exported.

- [ ] **Step 3: Wire integration dependencies**

Add:

```toml
[dependencies]
idiolectd = { path = "../idiolectd" }
```

Export from `crates/idiolectd/src/lib.rs`:

```rust
pub mod daemon;
```

- [ ] **Step 4: Run green command, leakage scan, and gates**

```bash
cargo test -p idiolect-integration-tests --test fake_dictation_loop
rg -n "rusqlite|fcitx|cpal|whisper|silero|opus|python|pytorch|peft" crates/idiolect-application crates/idiolectd crates/idiolect-integration-tests/tests/fake_dictation_loop.rs
bash ci/scripts/test-rust.sh
```

Expected: integration tests pass, `rg` emits no backend-leakage matches, and Rust gates pass with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock crates/idiolect-integration-tests crates/idiolectd docs/superpowers/plans/2026-06-04-idiolect-01-fake-dictation-loop.md
git commit -m "test: add fake dictation loop integration coverage"
```

## Rejection Criteria

Reject and rework this child if any condition holds:

```text
application code imports a real adapter crate
fake loop needs SQLite, CPAL, Whisper, Opus, Silero, Fcitx5, or Python
commit with the same idempotency key creates a second candidate
cancel after preedit creates a training candidate
daemon test requires a socket, filesystem database, or real input method process
any lint, compile, doc, or test warning appears
```

