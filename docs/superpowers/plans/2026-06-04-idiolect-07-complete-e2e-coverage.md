# Idiolect Complete E2E Coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add complete functional E2E coverage for the v1 process so Idiolect is not considered done until the whole dictation, learning, privacy, and packaging path is tested by release gates.

**Architecture:** Keep test harnesses at boundaries. `idiolectd` becomes the orchestrated runtime boundary, Rust integration tests drive daemon IPC and storage, C++ tests cover the Fcitx5 bridge, and CI scripts compose gates without bypassing warnings or using Python.

**Tech Stack:** Rust stable 1.96.0, Cargo workspace lints, Unix domain socket JSON Lines IPC, SQLite temp databases, deterministic audio fixtures, real Opus/VAD/Whisper adapters, CMake/CTest for C++, shell CI scripts.

---

## Current Verified Status

Historical gate snapshot logs from 2026-06-04 are retained only as baseline evidence, not as v1 completion proof.

Audit baseline noted on 2026-06-04:

- No `#[ignore]`, `#[allow(...)]`, disabled lint, or skipped required test was found in code paths during the final audit.
- `cargo-llvm-cov` is installed locally and the numeric Rust coverage gate is active.
- `ci/scripts/test-e2e.sh` drives fixture dictation, real media adapters, learning manifest/promotion/rollback, privacy deletion/exclusion, and the Fcitx5 IPC bridge.

## Completion Standard

This plan is complete only when all of the following are true:

- `ci/scripts/test-all.sh` exists and runs every required gate.
- `ci/scripts/test-e2e.sh` exists and runs the full process suites.
- `idiolectd` has a real testable runtime command, not only a crate-name smoke binary.
- A fixture full-stack E2E test proves preedit, correction, commit, storage, and candidate insertion through daemon IPC.
- A real media adapter E2E test proves fixture audio through Opus, VAD, Whisper, and daemon commit.
- A Fcitx5 bridge integration test proves the C++ side can speak IPC protocol version 1 and drive preedit commit/cancel behavior.
- A learning E2E test proves candidate capture feeds classifier, manifest generation, promotion, and rollback.
- A privacy E2E test proves export/delete behavior from a populated database and proves deleted data cannot enter future manifests.
- A package smoke test extracts the `.deb`, runs packaged binaries from the extracted root, and verifies package metadata and payload consistency.
- A coverage-map gate fails if any v1 process step lacks a named automated test.
- A numeric Rust coverage gate exists and fails below the agreed threshold.

## Agent Strategy

Use `gpt-5.3-codex-spark` for bounded mechanical tasks with exact file ownership:

- Task 1 coverage-map script and docs.
- Task 5 CLI privacy matrix tests.
- Task 6 trainer policy matrix tests.
- Task 8 package extraction smoke script.

Use a stronger model or gatekeeper-local work for integration-heavy tasks:

- Task 2 daemon runtime and IPC server.
- Task 3 fixture full-stack E2E.
- Task 4 real media full-stack E2E.
- Task 7 Fcitx5 bridge IPC integration.

Every worker must report exact red test command, green command, changed files, and lint gates. Gatekeeper must inspect diffs and rerun commands.

---

## Task 1: Coverage Map And All-Gates Orchestrator

**Owner:** Spark worker  
**Files:**
- Create: `docs/quality/v1-coverage-map.md`
- Create: `ci/scripts/test-coverage-map.sh`
- Create: `ci/scripts/test-all.sh`
- Modify: `README.md`

- [ ] **Step 1: Write failing coverage-map gate**

Create `ci/scripts/test-coverage-map.sh` that fails until every required row in `docs/quality/v1-coverage-map.md` is marked with an automated test id.

Required process rows:

```text
audio.capture
audio.fixture
codec.opus
vad.segment
asr.whisper
daemon.startup
ipc.handshake
ipc.lifecycle
fcitx5.preedit
fcitx5.commit
fcitx5.cancel
storage.event_log
storage.materialized_tables
candidate.capture
learning.classifier
learning.manifest
learning.promotion
learning.rollback
privacy.export
privacy.delete
privacy.deleted_data_excluded
package.payload
package.smoke
```

The script must reject any line containing `UNASSIGNED`.

- [ ] **Step 2: Run red command**

```bash
bash ci/scripts/test-coverage-map.sh
```

Expected: FAIL because `docs/quality/v1-coverage-map.md` does not exist or has `UNASSIGNED` rows.

- [ ] **Step 3: Add the coverage map and all-gates script**

Create `docs/quality/v1-coverage-map.md` with the rows above. Initially assign current known tests where valid and leave future rows assigned to the test names introduced later in this plan.

Create `ci/scripts/test-all.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

bash ci/scripts/test-rust.sh
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-integration.sh
bash ci/scripts/test-e2e.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-packaging.sh
bash ci/scripts/test-coverage-map.sh
bash ci/scripts/test-coverage.sh
```

Do not add `test-all.sh` to README as passing until Tasks 2-9 exist and pass.

- [ ] **Step 4: Run green command**

```bash
bash ci/scripts/test-coverage-map.sh
```

Expected: PASS with zero output.

- [ ] **Step 5: Commit**

```bash
git add README.md docs/quality/v1-coverage-map.md ci/scripts/test-coverage-map.sh ci/scripts/test-all.sh
git commit -m "ci: add v1 coverage map gate"
```

## Task 2: Testable Daemon Runtime And IPC Server

**Owner:** stronger model or gatekeeper-local  
**Files:**
- Create: `crates/idiolectd/src/runtime.rs`
- Create: `crates/idiolectd/tests/runtime_smoke.rs`
- Modify: `crates/idiolectd/src/lib.rs`
- Modify: `crates/idiolectd/src/main.rs`
- Modify: `crates/idiolectd/Cargo.toml`

- [ ] **Step 1: Write failing daemon runtime smoke test**

Create `crates/idiolectd/tests/runtime_smoke.rs` with tests that launch `env!("CARGO_BIN_EXE_idiolectd")`:

- `idiolectd_version_reports_json`
- `idiolectd_fixture_once_commits_to_temp_database`
- `idiolectd_fixture_once_cancel_records_no_candidate`

The command shape must be:

```bash
idiolectd --version --json
idiolectd fixture-once --db <temp-db> --transcript "restart traffic" --corrected "restart Traefik" --commit
idiolectd fixture-once --db <temp-db> --transcript "open notes" --cancel
```

Tests must assert JSON stdout and SQLite side effects using public repository query helpers.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolectd --test runtime_smoke
```

Expected: FAIL because `idiolectd` currently only prints the crate name.

- [ ] **Step 3: Implement minimal runtime**

Implement `idiolectd::runtime` with:

- `DaemonConfig { db_path, socket_path, mode }`
- `run_cli(args: &[String]) -> Result<String, RuntimeError>`
- `fixture_once` mode that uses the real SQLite adapter and existing dictation use case.
- `--version --json` returning package name, package version, and protocol version.

Do not expose backend types through `core`, `ports`, or `application`.

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolectd --test runtime_smoke
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolectd
git commit -m "feat: add testable daemon runtime"
```

## Task 3: Fixture Full-Stack Dictation E2E

**Owner:** stronger model or gatekeeper-local  
**Files:**
- Create: `crates/idiolect-integration-tests/tests/support/e2e.rs`
- Create: `crates/idiolect-integration-tests/tests/dictation_full_stack_fixture.rs`
- Create: `ci/scripts/test-e2e.sh`
- Modify: `crates/idiolect-integration-tests/Cargo.toml`
- Modify: `ci/scripts/test-integration.sh`

- [ ] **Step 1: Write failing full-stack fixture E2E test**

Create `dictation_full_stack_fixture.rs` with tests:

- `fixture_full_stack_commit_records_preedit_commit_storage_and_candidate`
- `fixture_full_stack_cancel_clears_preedit_and_records_no_candidate`
- `fixture_full_stack_duplicate_commit_is_idempotent`

The test must start daemon runtime on a temp Unix socket and temp SQLite DB, connect a Rust IPC client, send protocol v1 hello, send `StartRecording`, receive `PreeditUpdate`, send `CommitPreedit` or `CancelPreedit`, then assert:

- preedit text was sent,
- committed text is in materialized storage,
- exactly one candidate exists after commit,
- no candidate exists after cancel,
- duplicate commit does not create a second candidate.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-integration-tests --test dictation_full_stack_fixture
```

Expected: FAIL because no daemon IPC lifecycle server exists.

- [ ] **Step 3: Implement IPC lifecycle server behind runtime boundary**

Extend `idiolectd::runtime` with fixture IPC mode:

```bash
idiolectd serve-fixture --socket <socket> --db <db> --transcript "restart traffic"
```

It must speak `idiolect-ipc` JSON Lines over Unix domain sockets.

- [ ] **Step 4: Add E2E gate script**

Create `ci/scripts/test-e2e.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

cargo test -p idiolect-integration-tests --test dictation_full_stack_fixture --all-features
cargo test -p idiolect-integration-tests --test dictation_full_stack_real_adapters --all-features
cargo test -p idiolect-integration-tests --test learning_pipeline_manifest --all-features
cargo test -p idiolect-integration-tests --test privacy_e2e --all-features
bash ci/scripts/test-fcitx5-integration.sh
```

- [ ] **Step 5: Run green command and gates**

```bash
cargo test -p idiolect-integration-tests --test dictation_full_stack_fixture
bash ci/scripts/test-e2e.sh
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings after later referenced tests exist. During this task, `test-e2e.sh` may fail on not-yet-created later files; do not mark this task complete until either the script contains only existing suites or later tasks fill the missing suites before final verification.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolectd crates/idiolect-integration-tests ci/scripts/test-e2e.sh ci/scripts/test-integration.sh
git commit -m "test: add fixture full stack dictation e2e"
```

## Task 4: Real Media Adapter Full-Stack E2E

**Owner:** stronger model or gatekeeper-local  
**Files:**
- Create: `crates/idiolect-integration-tests/tests/dictation_full_stack_real_adapters.rs`
- Modify: `crates/idiolectd/src/runtime.rs`
- Modify: `tests/fixtures/whisper/README.md`

- [ ] **Step 1: Write failing real media E2E test**

Create `dictation_full_stack_real_adapters.rs` with:

- `real_media_full_stack_transcribes_fixture_and_commits_candidate`

The test must use deterministic fixture audio, real Opus codec, real VAD, real Whisper adapter, daemon orchestration, IPC lifecycle, and SQLite assertions.

It must not use a real microphone for required CI. Add a separate `cpal_virtual_audio_contract` only if the test can create a deterministic virtual source without skipped tests.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-integration-tests --test dictation_full_stack_real_adapters
```

Expected: FAIL because daemon runtime does not yet compose real media adapters into dictation IPC.

- [ ] **Step 3: Implement real media fixture mode**

Add runtime mode:

```bash
idiolectd serve-real-fixture --socket <socket> --db <db> --audio-fixture tests/fixtures/audio/restart_traffic_16khz_mono.wav --whisper-model tests/fixtures/whisper/ggml-tiny.en.bin
```

The mode must:

1. load fixture audio,
2. encode/decode through Opus when configured,
3. segment through VAD,
4. transcribe through Whisper,
5. send preedit through IPC,
6. commit/cancel through the dictation use case,
7. persist storage and candidate side effects.

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-integration-tests --test dictation_full_stack_real_adapters
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolectd crates/idiolect-integration-tests tests/fixtures/whisper/README.md
git commit -m "test: add real media full stack e2e"
```

## Task 5: CLI Matrix And Privacy E2E

**Owner:** Spark worker  
**Files:**
- Create: `crates/idiolect-cli/tests/cli_matrix.rs`
- Create: `crates/idiolect-integration-tests/tests/privacy_e2e.rs`
- Modify: `crates/idiolect-cli/tests/cli_privacy.rs`
- Modify: `crates/idiolect-adapter-sqlite/src/repository.rs`

- [ ] **Step 1: Write failing CLI matrix tests**

Add tests for:

- `doctor_requires_json`
- `privacy_export_requires_user`
- `privacy_export_requires_db`
- `privacy_delete_requires_user`
- `privacy_delete_requires_db`
- `privacy_delete_requires_confirm_delete`
- `unknown_privacy_argument_fails`

- [ ] **Step 2: Write failing privacy E2E test**

Create `privacy_e2e.rs` with:

- `privacy_delete_removes_training_data_and_future_manifest_excludes_user`

Populate SQLite through the dictation flow, run `idiolect-cli privacy export`, run `idiolect-cli privacy delete --confirm-delete`, then assert:

- training candidate count is zero,
- `UserDataDeleted` event exists,
- manifest generation for that user excludes deleted candidate data,
- privacy export reports zero candidates after deletion.

- [ ] **Step 3: Run red commands**

```bash
cargo test -p idiolect-cli --test cli_matrix
cargo test -p idiolect-integration-tests --test privacy_e2e
```

Expected: FAIL for missing tests/behavior.

- [ ] **Step 4: Implement missing CLI and repository behavior**

Keep CLI parsing dependency-free unless a plan amendment approves a parser crate. Add repository query methods only to adapter/private test surfaces where needed. Do not leak SQLite types into ports/application public APIs.

- [ ] **Step 5: Run green command and gates**

```bash
cargo test -p idiolect-cli --tests
cargo test -p idiolect-integration-tests --test privacy_e2e
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-cli crates/idiolect-integration-tests/tests/privacy_e2e.rs crates/idiolect-adapter-sqlite
git commit -m "test: add privacy and cli e2e coverage"
```

## Task 6: Learning Pipeline Manifest And Promotion E2E

**Owner:** Spark worker for tests, stronger model for any trainer wiring  
**Files:**
- Create: `crates/idiolect-integration-tests/tests/learning_pipeline_manifest.rs`
- Modify: `crates/idiolect-trainerctl/src/classifier.rs`
- Modify: `crates/idiolect-trainerctl/src/manifest.rs`
- Modify: `crates/idiolect-trainerctl/src/promotion.rs`
- Modify: `crates/idiolect-trainerctl/src/main.rs`

- [ ] **Step 1: Write failing learning pipeline E2E test**

Create `learning_pipeline_manifest.rs` with:

- `candidate_capture_classifier_manifest_promotion_and_rollback_are_connected`
- `deleted_user_data_is_excluded_from_manifest`
- `promotion_matrix_rejects_each_regression_reason`

The first test must use a candidate produced by dictation/storage, classify it, build a manifest, evaluate promotion, promote, then rollback.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-integration-tests --test learning_pipeline_manifest
```

Expected: FAIL because cross-component learning pipeline is not wired as an integration suite.

- [ ] **Step 3: Implement minimal trainer CLI/runtime wiring if needed**

If library functions are enough, do not add CLI. If CLI is needed, add commands:

```bash
idiolect-trainerctl manifest --db <db> --user <user> --output <path>
idiolect-trainerctl promote --manifest <path> --metrics <path>
idiolect-trainerctl rollback --target <adapter-id>
```

All commands must be covered by tests before implementation.

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-integration-tests --test learning_pipeline_manifest
cargo test -p idiolect-trainerctl --all-targets
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolect-integration-tests/tests/learning_pipeline_manifest.rs crates/idiolect-trainerctl
git commit -m "test: add learning pipeline e2e coverage"
```

## Task 7: Fcitx5 Bridge IPC Integration

**Owner:** stronger model or gatekeeper-local  
**Files:**
- Create: `fcitx5/idiolect-fcitx5/tests/e2e_ipc_bridge_test.cpp`
- Modify: `fcitx5/idiolect-fcitx5/CMakeLists.txt`
- Modify: `fcitx5/idiolect-fcitx5/src/ipc_client.cpp`
- Modify: `fcitx5/idiolect-fcitx5/src/ipc_client.h`
- Create: `ci/scripts/test-fcitx5-integration.sh`

- [ ] **Step 1: Write failing C++ IPC bridge test**

Create a CTest binary that:

- creates a temp Unix socket,
- starts a minimal test server or launches `idiolectd serve-fixture`,
- sends protocol v1 hello from the real C++ `IpcClient`,
- calls `start_recording`,
- receives or records `PreeditUpdate`,
- commits and cancels through the bridge,
- asserts protocol features are `preedit` and `commit`.

- [ ] **Step 2: Run red command**

```bash
bash ci/scripts/test-fcitx5-integration.sh
```

Expected: FAIL because no bridge integration script/test exists.

- [ ] **Step 3: Implement C++ bridge IPC behavior**

Keep the C++ shim thin. Do not duplicate business rules in C++. Business semantics stay in Rust daemon/application.

- [ ] **Step 4: Run green command and C++ gates**

```bash
bash ci/scripts/test-fcitx5-integration.sh
bash ci/scripts/test-fcitx5.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add fcitx5/idiolect-fcitx5 ci/scripts/test-fcitx5-integration.sh
git commit -m "test: add fcitx5 ipc bridge e2e"
```

## Task 8: Package Smoke And Version Consistency

**Owner:** Spark worker  
**Files:**
- Create: `ci/scripts/test-package-smoke.sh`
- Modify: `ci/scripts/test-packaging.sh`
- Modify: `packaging/debian/DEBIAN/control`
- Modify: `README.md`

- [ ] **Step 1: Write failing package smoke script**

`test-package-smoke.sh` must:

1. run `bash ci/scripts/test-packaging.sh`,
2. extract `target/package/idiolect_0.1.0_amd64.deb` into `target/package/smoke-root`,
3. run `target/package/smoke-root/usr/bin/idiolect-cli doctor --json`,
4. run `target/package/smoke-root/usr/bin/idiolectd --version --json`,
5. assert payload includes `idiolect-cli`, `idiolectd`, and `libidiolect-fcitx5.so`,
6. assert Debian version matches workspace package version.

- [ ] **Step 2: Run red command**

```bash
bash ci/scripts/test-package-smoke.sh
```

Expected: FAIL until the script and daemon version command exist.

- [ ] **Step 3: Implement smoke behavior**

Avoid root-only install steps in required CI. Use extraction smoke for required gates. If a real install/remove test is added, it must be a separate explicitly documented privileged gate, not silently skipped.

- [ ] **Step 4: Run green command and gates**

```bash
bash ci/scripts/test-package-smoke.sh
bash ci/scripts/test-packaging.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add ci/scripts/test-package-smoke.sh ci/scripts/test-packaging.sh packaging/debian/DEBIAN/control README.md
git commit -m "ci: add package smoke gate"
```

## Task 9: Numeric Rust Coverage Gate

**Owner:** Spark worker  
**Files:**
- Create: `ci/scripts/test-coverage.sh`
- Modify: `README.md`

- [ ] **Step 1: Write failing coverage script**

Create `test-coverage.sh` that requires `cargo-llvm-cov` and runs:

```bash
cargo llvm-cov --workspace --all-features --all-targets --fail-under-lines 80
```

The script must fail with a clear message if `cargo-llvm-cov` is not installed.

- [ ] **Step 2: Run red command**

```bash
bash ci/scripts/test-coverage.sh
```

Expected: FAIL in the current environment because `cargo-llvm-cov` is not installed.

- [ ] **Step 3: Install tooling or lower no gate**

Install `cargo-llvm-cov` outside this plan execution if needed. Do not replace this with a no-op. Do not skip the gate.

- [ ] **Step 4: Run green command**

```bash
bash ci/scripts/test-coverage.sh
```

Expected: PASS at or above 80% line coverage.

- [ ] **Step 5: Commit**

```bash
git add ci/scripts/test-coverage.sh README.md
git commit -m "ci: add rust coverage threshold gate"
```

## Task 10: Final Release Gate Reconciliation

**Owner:** gatekeeper-local  
**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-06-04-idiolect-v1-rust-first-implementation.md`
- Modify: `docs/superpowers/plans/2026-06-04-idiolect-07-complete-e2e-coverage.md`

- [ ] **Step 1: Run all required gates**

```bash
bash ci/scripts/test-all.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 2: Run direct release gates for verification-before-completion evidence**

```bash
bash ci/scripts/test-rust.sh
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-integration.sh
bash ci/scripts/test-e2e.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-packaging.sh
bash ci/scripts/test-package-smoke.sh
bash ci/scripts/test-coverage-map.sh
bash ci/scripts/test-coverage.sh
```

Expected: every command exits 0.

- [ ] **Step 3: Update docs**

README must list `test-all.sh` as the release gate and list direct gates below it.

Parent plan status must say child07 completed only after the commands above pass.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/superpowers/plans/2026-06-04-idiolect-v1-rust-first-implementation.md docs/superpowers/plans/2026-06-04-idiolect-07-complete-e2e-coverage.md
git commit -m "docs: mark e2e coverage complete"
```

## Final Acceptance Checklist

- [ ] Fresh `bash ci/scripts/test-all.sh` passes.
- [ ] Fresh direct gate list in Task 10 Step 2 passes.
- [ ] No skipped, ignored, or suppressed tests/lints exist.
- [ ] `cargo-llvm-cov` gate passes at or above 80% line coverage.
- [ ] Coverage map has no `UNASSIGNED` rows.
- [ ] At least one required E2E test covers the full fixture process.
- [ ] At least one required E2E test covers real Opus/VAD/Whisper media processing.
- [ ] Fcitx5 bridge has a C++ IPC integration test.
- [ ] Package smoke runs packaged binaries from extracted package root.
- [ ] Privacy delete excludes deleted data from future manifests.
- [ ] No Python dependency entered a v1 required path.
- [ ] No backend type leaked into `core`, `ports`, or `application` public APIs.
