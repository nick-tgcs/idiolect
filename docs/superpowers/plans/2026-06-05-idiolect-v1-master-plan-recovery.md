# Idiolect V1 Master Plan Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the current prototype back into alignment with `docs/idiolect_master_plan_rust_first.md` and prevent future false completion claims.

**Architecture:** Preserve ports-and-adapters. `idiolectd` becomes the only runtime composition root. Third-party libraries, including Burn, whisper-rs, Opus, CPAL, SQLite, and Fcitx5, stay behind Idiolect-owned interfaces. Gates must prove product behavior, not just row names or synthetic helper calls.

**Tech Stack:** Rust stable 1.96, Cargo workspace lints, Tokio for daemon runtime, TOML config, SQLite event log plus materialized tables, Ogg Opus audio store, whisper-rs ASR, CPAL audio capture, WebRTC VAD now with a future Silero/Burn path behind ports, Burn as the first Rust-native trainer backend candidate, CMake/CTest for Fcitx5, shell CI gates only.

---

## Gatekeeper Rules

- Gatekeeper owns architecture and final acceptance.
- Use `gpt-5.3-codex-spark` only for bounded mechanical tasks with exact file ownership.
- Use stronger model or gatekeeper-local work for daemon runtime, schema design, privacy semantics, trainer architecture, Fcitx5 behavior, and release-gate decisions.
- No implementation without an observed failing test first.
- Lint warnings are errors.
- No skipped, ignored, or suppressed tests/lints.
- No `#[allow(...)]` without a plan amendment approved before coding.
- No Python in runtime, trainer, promotion, packaging, or required test paths.
- No backend types in public APIs of `idiolect-core`, `idiolect-ports`, or `idiolect-application`.
- No claims of completion without fresh gate evidence.

## Required Gates After Every Rust Code Task

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

## Required C++ Gates After Fcitx5 Code Tasks

```bash
cmake -S fcitx5/idiolect-fcitx5 -B fcitx5/idiolect-fcitx5/build -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_CXX_FLAGS="-Wall -Wextra -Wpedantic -Werror"
cmake --build fcitx5/idiolect-fcitx5/build
ctest --test-dir fcitx5/idiolect-fcitx5/build --output-on-failure
```

## Subagent Strategy

Use Spark workers for:

- coverage-map and docs consistency scripts
- CLI parser matrix tests
- pure config structs and serde/TOML tests
- simple migration invariant tests after schema is designed
- metric matrix tests
- package content smoke scripts

Do not delegate to Spark:

- schema architecture decisions
- `idiolectd run` lifecycle design
- privacy delete/retention semantics
- Burn/trainer backend design
- Fcitx5 recovery behavior
- final gate acceptance

Every worker must return:

```text
Status:
Model used:
Files changed:
Red command and result:
Green command and result:
Lint/check/test commands and results:
Concerns:
```

## Milestones

### Task 1: Correct Status And Harden Evidence Gates

**Owner:** Spark worker for scripts/docs; gatekeeper review.

**Files:**
- Modify: `README.md`
- Modify: `ci/scripts/test-all.sh`
- Modify: `ci/scripts/test-coverage-map.sh`
- Modify: `docs/quality/v1-coverage-map.md`
- Modify: `docs/superpowers/plans/2026-06-04-idiolect-v1-rust-first-implementation.md`
- Modify: `docs/superpowers/plans/2026-06-04-idiolect-07-complete-e2e-coverage.md`

- [ ] **Step 1: Write failing status/gate tests**

Add a shell test command that fails because `test-all.sh` omits `test-package-smoke.sh` and because coverage map rows can point to nonexistent tests.

```bash
bash ci/scripts/test-coverage-map.sh
```

Expected first red after tightening: FAIL for at least one mapped test id that cannot be resolved to an executable test or script.

- [ ] **Step 2: Implement stricter coverage-map validation**

`ci/scripts/test-coverage-map.sh` must validate:

- every required process row exists exactly once
- no blank automated test value
- no `UNASSIGNED`
- script references exist and are executable
- Rust test ids are found by `rg "fn <test_id>" crates`
- C++ test ids are found by file target or `ctest -N`

- [ ] **Step 3: Correct release status text**

Docs must say current state is prototype baseline, not v1 complete.

- [ ] **Step 4: Run green commands**

```bash
bash ci/scripts/test-coverage-map.sh
bash ci/scripts/test-all.sh
```

- [ ] **Step 5: Commit**

```bash
git add README.md ci/scripts/test-all.sh ci/scripts/test-coverage-map.sh docs/quality/v1-coverage-map.md docs/superpowers/plans
git commit -m "ci: harden v1 evidence gates"
```

### Task 2: Configuration And XDG Runtime Layout

**Owner:** Spark for pure config tests; gatekeeper for runtime path policy.

**Files:**
- Modify: `crates/idiolect-common/src/config.rs`
- Modify: `crates/idiolect-common/src/lib.rs`
- Modify: `crates/idiolect-common/Cargo.toml`
- Create: `crates/idiolect-common/tests/config_contract.rs`
- Modify: `crates/idiolectd/src/runtime.rs`
- Create: `crates/idiolectd/tests/config_runtime.rs`

- [ ] **Step 1: Write failing config contract tests**

Test names:

- `config_defaults_match_master_plan`
- `config_rejects_empty_user_id`
- `config_resolves_xdg_paths_without_private_text`

Run:

```bash
cargo test -p idiolect-common --test config_contract
```

Expected: FAIL because config schema is missing.

- [ ] **Step 2: Implement config structs**

Implement sections:

- `[user]`
- `[daemon]`
- `[audio]`
- `[vad]`
- `[asr]`
- `[storage]`
- `[training]`
- `[privacy]`
- `[observability]`

Use typed fields and validation. Add TOML dependency only if pinned exactly in workspace dependencies.

- [ ] **Step 3: Write failing daemon config runtime tests**

Test names:

- `idiolectd_run_rejects_missing_model_path`
- `idiolectd_run_uses_configured_socket_and_database_paths`
- `idiolectd_run_does_not_log_private_text_by_default`

Run:

```bash
cargo test -p idiolectd --test config_runtime
```

Expected: FAIL because `idiolectd run` is missing.

- [ ] **Step 4: Wire config loading into daemon runtime**

Add:

```text
idiolectd run --config <path>
idiolectd config print-default --json
```

Do not start live capture in this task; validate config and path setup only.

- [ ] **Step 5: Run gates and commit**

```bash
cargo test -p idiolect-common --test config_contract
cargo test -p idiolectd --test config_runtime
bash ci/scripts/test-rust.sh
git add crates/idiolect-common crates/idiolectd Cargo.toml Cargo.lock
git commit -m "feat: add typed config and daemon path setup"
```

### Task 3: Full V1 Storage Schema And Audio Store

**Owner:** Gatekeeper or stronger model for schema; Spark for mechanical migration tests after design is locked.

**Files:**
- Create: `crates/idiolect-adapter-sqlite/migrations/0003_v1_storage.sql`
- Modify: `crates/idiolect-adapter-sqlite/src/migrations.rs`
- Modify: `crates/idiolect-adapter-sqlite/src/repository.rs`
- Modify: `crates/idiolect-ports/src/storage.rs`
- Create: `crates/idiolect-adapter-sqlite/tests/v1_schema_contract.rs`
- Create: `crates/idiolect-integration-tests/tests/audio_store_lifecycle.rs`

- [ ] **Step 1: Write failing schema tests**

Test names:

- `v1_schema_has_users_utterances_audio_and_session_links`
- `committed_session_links_exactly_one_utterance`
- `training_candidate_links_session_and_utterance`
- `delete_user_keeps_tombstone_but_removes_private_rows`

Run:

```bash
cargo test -p idiolect-adapter-sqlite --test v1_schema_contract
```

Expected: FAIL because `users`, `utterances`, and audio metadata links are missing.

- [ ] **Step 2: Implement migration**

Add tables/columns for:

- users
- utterances
- utterance_audio_files
- ime_text_sessions user/app/context fields
- ime_edit_events event type/cursor fields
- training_candidates user/utterance/split/status fields
- manifests and manifest_items
- adapter derivation records
- retention tombstones

- [ ] **Step 3: Write failing audio store lifecycle tests**

Test names:

- `opus_audio_file_is_written_and_reopened_for_training`
- `decoded_cache_is_deleted_on_privacy_delete`
- `retention_minimal_deletes_audio_after_classification`

Run:

```bash
cargo test -p idiolect-integration-tests --test audio_store_lifecycle
```

Expected: FAIL because audio file store does not exist.

- [ ] **Step 4: Implement audio store adapter behind a port**

Add an Idiolect-owned object/audio store port before any filesystem-specific implementation leaks into application/core APIs.

- [ ] **Step 5: Run gates and commit**

```bash
cargo test -p idiolect-adapter-sqlite --test v1_schema_contract
cargo test -p idiolect-integration-tests --test audio_store_lifecycle
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-rust.sh
git add crates/idiolect-adapter-sqlite crates/idiolect-ports crates/idiolect-integration-tests
git commit -m "feat: add v1 storage and audio lifecycle"
```

### Task 4: Real `idiolectd run` Daemon

**Owner:** Gatekeeper or stronger model.

**Files:**
- Modify: `crates/idiolectd/src/runtime.rs`
- Create: `crates/idiolectd/src/run_loop.rs`
- Create: `crates/idiolectd/src/adapters.rs`
- Create: `crates/idiolectd/tests/run_loop_smoke.rs`
- Create: `crates/idiolect-integration-tests/tests/daemon_run_lifecycle.rs`

- [ ] **Step 1: Write failing binary tests**

Test names:

- `idiolectd_run_starts_socket_and_accepts_hello`
- `idiolectd_run_rejects_second_instance_on_same_socket`
- `idiolectd_run_shutdown_cleans_socket_file`

Run:

```bash
cargo test -p idiolectd --test run_loop_smoke
```

Expected: FAIL because `run` is not a known command.

- [ ] **Step 2: Implement daemon run loop**

Implement:

- `idiolectd run --config <path>`
- Unix socket bind from config
- protocol negotiation
- one session per client to start
- graceful shutdown for tests
- safe error messages without private text

- [ ] **Step 3: Write failing integration lifecycle tests**

Test names:

- `daemon_run_fixture_audio_preedit_commit_persists_audio_session_and_candidate`
- `daemon_run_cancel_does_not_commit_text`
- `daemon_run_retry_does_not_duplicate_committed_session`
- `daemon_disconnect_marks_session_abandoned`

Run:

```bash
cargo test -p idiolect-integration-tests --test daemon_run_lifecycle
```

Expected: FAIL until daemon run loop composes ports.

- [ ] **Step 4: Compose fixture and real adapters**

Support test-mode config profiles:

- fixture audio
- real CPAL capture disabled in CI by default
- real VAD
- real Opus
- real whisper-rs fixture model
- SQLite metadata store
- audio store

- [ ] **Step 5: Run gates and commit**

```bash
cargo test -p idiolectd --test run_loop_smoke
cargo test -p idiolect-integration-tests --test daemon_run_lifecycle
bash ci/scripts/test-rust.sh
git add crates/idiolectd crates/idiolect-integration-tests
git commit -m "feat: implement daemon run loop"
```

### Task 5: Fcitx5 Product Integration And Recovery

**Owner:** Stronger model or gatekeeper for behavior; Spark for C++ parser tests only.

**Files:**
- Modify: `fcitx5/idiolect-fcitx5/src/engine.cpp`
- Modify: `fcitx5/idiolect-fcitx5/src/engine.h`
- Modify: `fcitx5/idiolect-fcitx5/src/ipc_client.cpp`
- Modify: `fcitx5/idiolect-fcitx5/src/ipc_client.h`
- Create: `fcitx5/idiolect-fcitx5/data/org.fcitx.Fcitx5.Addon.Idiolect.metainfo.xml`
- Create: `fcitx5/idiolect-fcitx5/data/idiolect.conf`
- Create: `fcitx5/idiolect-fcitx5/data/idiolect-addon.conf`
- Create: `fcitx5/idiolect-fcitx5/tests/disconnect_recovery_test.cpp`
- Create: `crates/idiolect-integration-tests/tests/fcitx5_daemon_recovery.rs`

- [ ] **Step 1: Write failing C++ recovery tests**

Run:

```bash
cmake -S fcitx5/idiolect-fcitx5 -B fcitx5/idiolect-fcitx5/build -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_CXX_FLAGS="-Wall -Wextra -Wpedantic -Werror"
cmake --build fcitx5/idiolect-fcitx5/build
ctest --test-dir fcitx5/idiolect-fcitx5/build --output-on-failure
```

Expected: FAIL for missing disconnect recovery test target.

- [ ] **Step 2: Implement thin recovery behavior**

Fcitx5 shim must:

- clear or preserve preedit according to configured safe default on daemon disconnect
- not duplicate business logic
- reconnect without corrupting current app text

- [ ] **Step 3: Add install metadata**

Package-visible metadata must exist before package tests can pass.

- [ ] **Step 4: Run gates and commit**

```bash
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-fcitx5-integration.sh
git add fcitx5 crates/idiolect-integration-tests
git commit -m "feat: add fcitx5 recovery and install metadata"
```

### Task 6: Product CLI Surface

**Owner:** Spark for parser matrix; gatekeeper for command semantics.

**Files:**
- Modify: `crates/idiolect-cli/src/lib.rs`
- Create: `crates/idiolect-cli/tests/product_commands.rs`
- Create: `crates/idiolect-cli/tests/doctor_health.rs`
- Modify: `README.md`

- [ ] **Step 1: Write failing CLI matrix tests**

Test command:

```bash
cargo test -p idiolect-cli --test product_commands
```

Expected: FAIL for missing commands.

Required command groups:

- `doctor`
- `service status`
- `service restart`
- `models list`
- `models install <model-id>`
- `sessions list/show/delete`
- `memory list/delete`
- `candidates list`
- `train export-manifest`
- `train classify`
- `train run`
- `adapters list/promote/rollback`
- `privacy export`
- `privacy delete-all`

- [ ] **Step 2: Implement command parser and safe stubs**

Commands that cannot act yet must return typed `not implemented` JSON and nonzero exit until their backing task lands. Do not fake success.

- [ ] **Step 3: Replace hardcoded doctor**

Doctor must check configured paths, database migrations, socket reachability, model fixture presence, and Fcitx5 metadata presence.

- [ ] **Step 4: Run gates and commit**

```bash
cargo test -p idiolect-cli --tests
bash ci/scripts/test-rust.sh
git add crates/idiolect-cli README.md
git commit -m "feat: add product cli command surface"
```

### Task 7: Manifest V2, Splits, And Training Inputs

**Owner:** Gatekeeper or stronger model.

**Files:**
- Modify: `crates/idiolect-trainerctl/src/manifest.rs`
- Modify: `crates/idiolect-adapter-sqlite/src/repository.rs`
- Create: `crates/idiolect-trainerctl/tests/manifest_v2.rs`
- Create: `crates/idiolect-integration-tests/tests/manifest_builder_storage.rs`

- [ ] **Step 1: Write failing manifest V2 tests**

Test names:

- `manifest_v2_contains_train_validation_and_holdout_splits`
- `holdout_item_never_appears_in_training_split`
- `manifest_item_links_audio_and_text_session`
- `manifest_digest_changes_when_audio_digest_changes`

Run:

```bash
cargo test -p idiolect-trainerctl --test manifest_v2
```

Expected: FAIL because manifest V2 does not exist.

- [ ] **Step 2: Implement manifest V2 domain**

Manifest must include:

- user id
- utterance id
- session id
- audio path or object key
- audio digest
- raw transcript
- corrected transcript
- split
- source label
- trust score
- base model id

- [ ] **Step 3: Run gates and commit**

```bash
cargo test -p idiolect-trainerctl --test manifest_v2
cargo test -p idiolect-integration-tests --test manifest_builder_storage
bash ci/scripts/test-rust.sh
git add crates/idiolect-trainerctl crates/idiolect-adapter-sqlite crates/idiolect-integration-tests
git commit -m "feat: add manifest v2 with splits"
```

### Task 8: Rust-Native Trainer Backend With Burn Candidate

**Owner:** Gatekeeper/stronger model for design; Spark may handle deterministic fake backend tests.

**Files:**
- Create: `crates/idiolect-ml-core/Cargo.toml`
- Create: `crates/idiolect-ml-core/src/lib.rs`
- Create: `crates/idiolect-ml-core/src/artifact.rs`
- Create: `crates/idiolect-ml-core/src/metrics.rs`
- Create: `crates/idiolect-trainer-burn/Cargo.toml`
- Create: `crates/idiolect-trainer-burn/src/lib.rs`
- Create: `crates/idiolect-trainer-burn/src/trainer.rs`
- Modify: `Cargo.toml`
- Modify: `crates/idiolect-ports/src/trainer.rs`
- Modify: `crates/idiolect-ports/src/evaluator.rs`
- Create: `crates/idiolect-trainer-burn/tests/burn_trainer_contract.rs`

- [ ] **Step 1: Decide exact Burn version**

Before code, check the local Cargo cache or official crate metadata if network is available. Pin exact version in workspace dependencies. Do not use wildcard versions.

- [ ] **Step 2: Write failing trainer contract tests**

Test names:

- `burn_trainer_consumes_manifest_and_emits_candidate_artifact`
- `candidate_artifact_records_base_model_manifest_and_backend`
- `trainer_rejects_manifest_without_audio`

Run:

```bash
cargo test -p idiolect-trainer-burn --test burn_trainer_contract
```

Expected: FAIL because crate does not exist.

- [ ] **Step 3: Implement minimal deterministic Burn-backed trainer**

The first implementation may train a tiny deterministic fixture model or emit a Burn-owned adapter artifact for a small fixture dataset. It must use Burn through the trainer adapter crate only and return Idiolect-owned `TrainingArtifact`.

- [ ] **Step 4: Add fake trainer contract**

Any shared `TrainerPort` contract must run against both a fake trainer and Burn trainer.

- [ ] **Step 5: Run gates and commit**

```bash
cargo test -p idiolect-trainer-burn --test burn_trainer_contract
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-rust.sh
git add Cargo.toml Cargo.lock crates/idiolect-ml-core crates/idiolect-trainer-burn crates/idiolect-ports
git commit -m "feat: add burn trainer backend contract"
```

### Task 9: Evaluation, Adapter Registry, Promotion, And Rollback Persistence

**Owner:** Gatekeeper or stronger model.

**Files:**
- Modify: `crates/idiolect-trainerctl/src/metrics.rs`
- Modify: `crates/idiolect-trainerctl/src/promotion.rs`
- Modify: `crates/idiolect-adapter-sqlite/src/repository.rs`
- Create: `crates/idiolect-trainerctl/tests/evaluation_matrix.rs`
- Create: `crates/idiolect-integration-tests/tests/adapter_registry_persistence.rs`

- [ ] **Step 1: Write failing evaluation tests**

Test names:

- `evaluation_report_contains_master_plan_metrics`
- `promotion_rejects_command_regression`
- `promotion_rejects_deletion_rate_regression`
- `promotion_rejects_realtime_factor_regression`

Run:

```bash
cargo test -p idiolect-trainerctl --test evaluation_matrix
```

Expected: FAIL because metrics are missing.

- [ ] **Step 2: Write failing persistence tests**

Test names:

- `adapter_registry_persists_current_previous_best_and_historical`
- `promotion_is_atomic_on_storage_failure`
- `rollback_restores_previous_active_adapter_after_restart`
- `deleted_sample_marks_derived_adapter`

Run:

```bash
cargo test -p idiolect-integration-tests --test adapter_registry_persistence
```

Expected: FAIL because registry is in-memory.

- [ ] **Step 3: Implement persistent registry**

Implement state transitions:

- candidate
- active
- previous
- best
- rejected
- rollback target
- derived from deleted sample

- [ ] **Step 4: Run gates and commit**

```bash
cargo test -p idiolect-trainerctl --test evaluation_matrix
cargo test -p idiolect-integration-tests --test adapter_registry_persistence
bash ci/scripts/test-rust.sh
git add crates/idiolect-trainerctl crates/idiolect-adapter-sqlite crates/idiolect-integration-tests
git commit -m "feat: persist adapter evaluation and rollback"
```

### Task 10: Privacy, Retention, And Observability

**Owner:** Gatekeeper for privacy semantics; Spark for CLI matrix tests.

**Files:**
- Modify: `crates/idiolect-cli/src/lib.rs`
- Modify: `crates/idiolect-adapter-sqlite/src/repository.rs`
- Modify: `crates/idiolectd/src/runtime.rs`
- Create: `crates/idiolect-integration-tests/tests/privacy_retention.rs`
- Create: `crates/idiolect-cli/tests/observability_privacy.rs`

- [ ] **Step 1: Write failing privacy tests**

Test names:

- `privacy_delete_removes_audio_text_events_candidates_cache_and_manifest_refs`
- `strict_privacy_excludes_deleted_sample_from_future_adapter`
- `normal_typing_outside_idiolect_session_is_not_stored`
- `logs_do_not_include_private_text_by_default`

Run:

```bash
cargo test -p idiolect-integration-tests --test privacy_retention
```

Expected: FAIL because audio/cache/artifact lifecycle is missing.

- [ ] **Step 2: Implement retention modes**

Modes:

- minimal
- balanced
- research

Implement tombstones, decoded cache deletion, audio deletion by mode, and adapter derivation marking.

- [ ] **Step 3: Implement observability commands**

Commands:

- `idiolect doctor --audio --json`
- `idiolect doctor --fcitx5 --json`
- `idiolect logs show`
- `idiolect logs show --include-private`

Private text must require explicit flag.

- [ ] **Step 4: Run gates and commit**

```bash
cargo test -p idiolect-integration-tests --test privacy_retention
cargo test -p idiolect-cli --test observability_privacy
bash ci/scripts/test-rust.sh
git add crates/idiolect-cli crates/idiolect-adapter-sqlite crates/idiolectd crates/idiolect-integration-tests
git commit -m "feat: add privacy retention and redacted observability"
```

### Task 11: Packaging Install Lifecycle

**Owner:** Stronger model for package lifecycle; Spark for content scripts.

**Files:**
- Modify: `ci/scripts/test-packaging.sh`
- Modify: `ci/scripts/test-package-smoke.sh`
- Create: `ci/scripts/test-package-lifecycle.sh`
- Modify: `packaging/debian/DEBIAN/control`
- Create: `packaging/debian/usr/lib/systemd/user/idiolectd.service`
- Add Fcitx5 metadata from Task 5 into package paths

- [ ] **Step 1: Write failing package lifecycle gate**

Run:

```bash
bash ci/scripts/test-package-lifecycle.sh
```

Expected: FAIL because service, metadata, install, enable, disable, upgrade, and uninstall checks do not exist.

- [ ] **Step 2: Add package payload**

Package must include:

- `idiolectd`
- user-facing `idiolect`
- optional `idiolect-train`
- Fcitx5 shared library
- Fcitx5 metadata
- systemd user service
- sample config
- docs

- [ ] **Step 3: Add lifecycle tests**

Where real `dpkg -i` is unsafe, use a container or fakeroot strategy. The gate must not claim full packaging complete until clean-VM install/uninstall evidence exists.

- [ ] **Step 4: Run gates and commit**

```bash
bash ci/scripts/test-packaging.sh
bash ci/scripts/test-package-smoke.sh
bash ci/scripts/test-package-lifecycle.sh
git add ci/scripts packaging
git commit -m "ci: add package lifecycle gate"
```

### Task 12: All-Done Acceptance E2E Gates

**Owner:** Gatekeeper for scope; Spark for mechanical gate scripts.

**Files:**
- Create: `ci/scripts/test-e2e-headless.sh`
- Create: `ci/scripts/test-e2e-failure-recovery.sh`
- Create: `ci/scripts/test-model-regression.sh`
- Create: `ci/scripts/test-performance.sh`
- Modify: `ci/scripts/test-all.sh`
- Modify: `docs/quality/v1-coverage-map.md`
- Create: `docs/quality/v1-acceptance-evidence.md`

- [ ] **Step 1: Write failing acceptance evidence gate**

`docs/quality/v1-acceptance-evidence.md` must map every row in master sections 26.4, 26.5, and 29.17 to a command and test id.

Run:

```bash
bash ci/scripts/test-coverage-map.sh
```

Expected: FAIL until all acceptance rows have executable evidence.

- [ ] **Step 2: Add failure recovery tests**

Scenarios:

- daemon crash after audio capture before commit
- Fcitx5 disconnect during preedit
- disk full during commit
- ASR empty transcript
- ASR low-confidence transcript
- model load failure

- [ ] **Step 3: Add app matrix plan and gate**

At minimum:

- Firefox
- Chromium
- terminal
- GTK editor
- Qt editor
- VS Code/Electron

If not practical in PR CI, mark as nightly/manual gate and keep v1 incomplete until evidence exists.

- [ ] **Step 4: Add model and performance gates**

Model gate must cover fixture audio and expected transcript/latency thresholds. Performance gate must record startup latency, transcription latency, and memory footprint.

- [ ] **Step 5: Run gates and commit**

```bash
bash ci/scripts/test-e2e-failure-recovery.sh
bash ci/scripts/test-model-regression.sh
bash ci/scripts/test-performance.sh
bash ci/scripts/test-coverage-map.sh
git add ci/scripts docs/quality
git commit -m "ci: add all-done acceptance gates"
```

## Completion Standard

This recovery plan is complete only when:

- `idiolectd run` starts a real daemon and passes socket lifecycle tests.
- Fcitx5 can drive preedit, correction, commit, cancel, retry, disconnect recovery, and app matrix tests.
- Audio capture, VAD, ASR, Opus storage, text session, correction capture, candidate creation, classifier, manifest, trainer, evaluation, promotion, rollback, export, and delete are connected end to end.
- Burn exists as a real Rust-native trainer backend candidate behind `TrainerPort`.
- Packaging install/enable/disable/upgrade/uninstall evidence exists.
- Every master-plan acceptance row has executable evidence.
- All required Rust and C++ gates pass freshly.
