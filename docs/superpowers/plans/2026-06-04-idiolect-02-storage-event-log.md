# Idiolect 02 Storage Event Log Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement SQLite-backed storage with immutable migrations, append-only `event_log`, materialized read tables, idempotent repository writes, and restart-safe lifecycle tests.

**Architecture:** `idiolect-adapter-sqlite` implements `MetadataStorePort` and owns all SQLite details. SQLite and checksum crates are adapter details and must not leak into `idiolect-core`, `idiolect-ports`, or `idiolect-application` public APIs.

**Tech Stack:** Rust, `rusqlite = "=0.40.0"` with `default-features = false` and `features = ["bundled"]`, `sha2 = "=0.11.0"`, SQLite SQL migrations, strict Cargo lint gates.

---

## Scope Boundary

Allowed behavior:

```text
SQLite adapter crate
migration catalog
schema_migrations checksum enforcement
append-only event_log
materialized tables for sessions, edits, candidates, adapters, training runs, correction memory
storage lifecycle integration tests
```

Forbidden behavior:

```text
real audio capture
real ASR
real VAD
real codec
Fcitx5 shim changes
trainer policy changes
Python required-path code
mutation of an accepted migration after this child is accepted without a new migration file
```

Required gates after every code task:

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

## Migration Immutability Rule

`crates/idiolect-adapter-sqlite/migrations/0001_initial.sql` and `0002_correction_memory.sql` become immutable after this child is accepted. Any later schema change must use a new higher-numbered migration. The migration runner must compare stored checksums against embedded checksums before deciding a migration is already applied.

## Verified Dependency Choices

The gatekeeper ran these commands on 2026-06-04:

```bash
cargo search rusqlite --limit 1
cargo search sha2 --limit 1
cargo info rusqlite@0.40.0 -v
```

Results selected for this child:

```toml
rusqlite = { version = "=0.40.0", default-features = false, features = ["bundled"] }
sha2 = "=0.11.0"
```

`rusqlite` remains confined to `idiolect-adapter-sqlite`; `sha2` is only for migration checksums.

## Task 1: Dependency Decision And Crate Wiring

**Owner:** Gatekeeper-local for the decision record, Spark worker allowed for mechanical code  
**Model:** Gatekeeper-local for dependency choice; `gpt-5.3-codex-spark` for tests and manifest wiring  
**Files:**

- Create: `docs/decisions/0002-sqlite-storage-adapter.md`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/idiolect-adapter-sqlite/Cargo.toml`
- Create: `crates/idiolect-adapter-sqlite/tests/dependency_contract.rs`

- [ ] **Step 1: Write the decision record**

Create `docs/decisions/0002-sqlite-storage-adapter.md` recording:

```text
rusqlite = =0.40.0
rusqlite default-features = false
rusqlite features = ["bundled"]
sha2 = =0.11.0
SQLite and checksum crate types are adapter-private and remain behind MetadataStorePort.
```

- [ ] **Step 2: Write failing dependency smoke test**

Create `crates/idiolect-adapter-sqlite/tests/dependency_contract.rs`:

```rust
use rusqlite::Connection;
use sha2::{Digest, Sha256};

#[test]
fn sqlite_and_checksum_dependencies_are_available() {
    let connection = Connection::open_in_memory().expect("sqlite should open in memory");
    connection
        .execute_batch("CREATE TABLE smoke(id INTEGER PRIMARY KEY);")
        .expect("sqlite should execute smoke schema");

    let digest = Sha256::digest(b"idiolect-storage-smoke");
    assert_eq!(format!("{digest:x}").len(), 64);
}
```

- [ ] **Step 3: Run red command**

```bash
cargo test -p idiolect-adapter-sqlite --test dependency_contract
```

Expected: FAIL because `rusqlite` and `sha2` are not yet dependencies.

- [ ] **Step 4: Add exact pinned dependencies**

Add to root `[workspace.dependencies]`:

```toml
rusqlite = { version = "=0.40.0", default-features = false, features = ["bundled"] }
sha2 = "=0.11.0"
```

Add to `crates/idiolect-adapter-sqlite/Cargo.toml`:

```toml
[dependencies]
idiolect-common = { path = "../idiolect-common" }
idiolect-ports = { path = "../idiolect-ports" }
rusqlite.workspace = true
sha2.workspace = true
```

- [ ] **Step 5: Run green command and gates**

```bash
cargo test -p idiolect-adapter-sqlite --test dependency_contract
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add docs/decisions/0002-sqlite-storage-adapter.md Cargo.toml Cargo.lock crates/idiolect-adapter-sqlite
git commit -m "feat: wire sqlite adapter dependencies"
```

## Task 2: Initial Event-Log Migration And Catalog

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Create: `crates/idiolect-adapter-sqlite/migrations/0001_initial.sql`
- Create: `crates/idiolect-adapter-sqlite/src/migrations.rs`
- Create: `crates/idiolect-adapter-sqlite/src/repository.rs`
- Modify: `crates/idiolect-adapter-sqlite/src/lib.rs`
- Create: `crates/idiolect-adapter-sqlite/tests/migration_contract.rs`
- Create: `crates/idiolect-adapter-sqlite/tests/repository_contract.rs`

- [ ] **Step 1: Write failing migration catalog and schema tests**

`migration_contract.rs` must assert:

```text
migrations() versions are [1]
version 1 name is initial
sha256_hex() equals expected_sha256_hex
migration_by_version(99) is None
```

`repository_contract.rs` must include `migration_01_creates_event_log` and `migration_01_creates_materialized_tables`. Use `SqliteMetadataStore::open_in_memory()`, call `migrate()`, assert `event_log` has `id`, `aggregate_type`, `aggregate_id`, `event_type`, `event_version`, `event_json`, `idempotency_key`, `created_at`, `created_by`, and assert materialized tables exist.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-adapter-sqlite --test migration_contract
cargo test -p idiolect-adapter-sqlite --test repository_contract
```

Expected: FAIL because the migration catalog, SQL file, and repository are absent.

- [ ] **Step 3: Create `0001_initial.sql`**

The SQL must create:

```text
schema_migrations(version, name, applied_at, checksum)
event_log(id, aggregate_type, aggregate_id, event_type, event_version, event_json, idempotency_key, created_at, created_by)
ime_text_sessions(id, raw_stt_text, committed_text, state, created_at, committed_at, cancelled_at)
ime_edit_events(id, session_id, from_text, to_text, event_index, created_at)
training_candidates(id, session_id, raw_text, corrected_text, source, trust_score, capture_quality, idempotency_key, created_at)
adapters(id, user_id, artifact_digest, manifest_digest, metric_report_digest, active, created_at, promoted_at)
training_runs(id, user_id, manifest_digest, status, started_at, finished_at)
```

Add indexes for aggregate lookup, idempotency uniqueness, edit event ordering, session candidate lookup, active adapter lookup, and training-run user lookup.

- [ ] **Step 4: Implement migration catalog and minimal repository migration entry point**

Implement:

```rust
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
    pub expected_sha256_hex: &'static str,
}

impl Migration {
    pub fn sha256_hex(&self) -> String;
}

pub fn migrations() -> &'static [Migration];
pub fn migration_by_version(version: i64) -> Option<&'static Migration>;
```

Use `include_str!` for `0001_initial.sql`. Compute and paste the exact SHA-256 value after final SQL bytes are in place.

`SqliteMetadataStore::open_in_memory()` opens an in-memory SQLite connection. `migrate()` applies migration 1 in a transaction, records `schema_migrations` only after the migration succeeds, and validates stored checksums before treating any migration as already applied. Expose test helpers for `table_exists_for_test`, `table_columns_for_test`, `applied_migration_versions_for_test`, `schema_migration_rows_for_test`, and `force_schema_checksum_for_test`.

- [ ] **Step 5: Refresh checksum constants and run green command**

```bash
sha256sum crates/idiolect-adapter-sqlite/migrations/0001_initial.sql
cargo test -p idiolect-adapter-sqlite --test migration_contract
cargo test -p idiolect-adapter-sqlite --test repository_contract
```

Expected: PASS and checksum tests match embedded constants.

- [ ] **Step 6: Run gates and commit**

```bash
bash ci/scripts/test-rust.sh
git add crates/idiolect-adapter-sqlite
git commit -m "feat: add sqlite initial event-log schema"
```

## Task 3: Correction Memory Migration

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Create: `crates/idiolect-adapter-sqlite/migrations/0002_correction_memory.sql`
- Modify: `crates/idiolect-adapter-sqlite/tests/migration_contract.rs`
- Modify: `crates/idiolect-adapter-sqlite/tests/repository_contract.rs`
- Modify: `crates/idiolect-adapter-sqlite/src/migrations.rs`

- [ ] **Step 1: Write failing correction-memory tests**

Update migration contract to expect versions `[1, 2]` and names `initial`, `correction_memory`. Add tests `migration_02_adds_correction_memory` and `migration_02_is_recorded_after_01` in `repository_contract.rs`. They must migrate an in-memory store, assert `correction_memory` exists with `raw_text`, `corrected_text`, and `occurrence_count`, and assert applied versions are `[1, 2]`.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-adapter-sqlite --test migration_contract
cargo test -p idiolect-adapter-sqlite --test repository_contract
```

Expected: FAIL because migration 2 is absent.

- [ ] **Step 3: Create `0002_correction_memory.sql`**

The SQL must create:

```text
correction_memory(id, raw_text, corrected_text, confidence, occurrence_count, first_seen_at, last_seen_at)
unique index on raw_text and corrected_text
lookup index on raw_text
lookup index on corrected_text
```

- [ ] **Step 4: Refresh checksum constants and run green command**

```bash
sha256sum crates/idiolect-adapter-sqlite/migrations/0001_initial.sql crates/idiolect-adapter-sqlite/migrations/0002_correction_memory.sql
cargo test -p idiolect-adapter-sqlite --test migration_contract
cargo test -p idiolect-adapter-sqlite --test repository_contract
```

Expected: PASS with fixed checksum constants.

- [ ] **Step 5: Run gates and commit**

```bash
bash ci/scripts/test-rust.sh
git add crates/idiolect-adapter-sqlite
git commit -m "feat: add correction memory migration"
```

## Task 4: Migration Runner Verification Checkpoint

**Owner:** Gatekeeper-local  
**Model:** Gatekeeper-local  
**Files:**

- No source edits expected. Amend the plan before code if this checkpoint finds a gap.

- [ ] **Step 1: Re-run strict migration-runner tests**

Task 2 owns checksum enforcement and migration idempotency because the child-level immutability rule applies from the first runner implementation. At this checkpoint, verify `repository_contract.rs` already covers `migrate_is_idempotent` and `migrate_with_mismatched_checksum_fails_fast`.

- [ ] **Step 2: Run verification commands**

```bash
cargo test -p idiolect-adapter-sqlite --test repository_contract
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 3: Commit**

No commit is expected unless the checkpoint required a plan amendment or a missing test was found and fixed with a fresh red/green cycle.

## Task 5: Idempotent Repository Writes

**Owner:** Spark worker allowed, gatekeeper reviews storage semantics  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-adapter-sqlite/Cargo.toml`
- Modify: `crates/idiolect-adapter-sqlite/src/repository.rs`
- Modify: `crates/idiolect-adapter-sqlite/tests/repository_contract.rs`

- [ ] **Step 1: Write failing repository behavior tests**

Add tests `commit_session_is_idempotent_with_same_key`, `duplicate_idempotency_key_with_different_payload_is_conflict`, and `cancel_session_after_commit_does_not_change_committed_row`. Use `MetadataStorePort`, a migrated in-memory store, and helper assertions for event count, candidate count, and session state.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-adapter-sqlite --test repository_contract
```

Expected: FAIL because `MetadataStorePort` is not implemented by `SqliteMetadataStore`.

- [ ] **Step 3: Implement repository writes**

Use the existing `ImeSessionId` Rust serialization interface for SQLite row keys by adding `serde_json.workspace = true` to the adapter manifest. Do not use debug formatting for persisted IDs.

Every write occurs in one transaction:

```text
create_session writes SessionCreated and inserts ime_text_sessions row
record_preedit_change writes PreeditCorrected and inserts ime_edit_events row
commit_session requires an existing session, writes SessionCommitted, and updates committed state plus one training_candidates row
cancel_session writes SessionCancelled and sets cancelled state only when session is not committed
same idempotency key and same payload returns success without extra writes
different payload for an existing idempotency key returns IdempotencyConflict
```

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-adapter-sqlite --test repository_contract
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock docs/superpowers/plans/2026-06-04-idiolect-02-storage-event-log.md crates/idiolect-adapter-sqlite/Cargo.toml crates/idiolect-adapter-sqlite/src/repository.rs crates/idiolect-adapter-sqlite/tests/repository_contract.rs
git commit -m "feat: implement idempotent sqlite metadata writes"
```

## Task 6: Storage Lifecycle Integration

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `Cargo.lock`
- Modify: `crates/idiolect-adapter-sqlite/src/repository.rs`
- Modify: `crates/idiolect-integration-tests/Cargo.toml`
- Create: `crates/idiolect-integration-tests/tests/storage_lifecycle.rs`

- [ ] **Step 1: Write failing lifecycle tests**

Create tests `committed_session_writes_event_then_materialized_rows` and `lifecycle_commit_is_replay_consistent_after_restart`. Use a temporary database path from `std::env::temp_dir()`. Add a minimal SQLite adapter path opener such as `SqliteMetadataStore::open_path` after observing the red test failure; do not add new dependencies. Migrate, create a session, commit, reopen, migrate again, and assert the committed state and one training candidate survive restart.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-integration-tests --test storage_lifecycle
```

Expected: FAIL until integration dependencies are wired.

- [ ] **Step 3: Wire integration dependencies**

Add these dependencies while preserving any existing `[dependencies]` entries:

```toml
idiolect-adapter-sqlite = { path = "../idiolect-adapter-sqlite" }
idiolect-ports = { path = "../idiolect-ports" }
```

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-integration-tests --test storage_lifecycle
cargo test -p idiolect-adapter-sqlite
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock docs/superpowers/plans/2026-06-04-idiolect-02-storage-event-log.md crates/idiolect-adapter-sqlite/src/repository.rs crates/idiolect-integration-tests
git commit -m "test: add sqlite storage lifecycle integration coverage"
```

## Rejection Criteria

Reject and rework this child if any condition holds:

```text
any dependency version is wildcard or range-only without an exact selected version in the decision record
0001_initial.sql or 0002_correction_memory.sql changes after this child is accepted instead of a new migration file
schema_migrations checksum mismatch is ignored
migrate called twice changes schema_migrations rows
event append succeeds while materialized table update fails
same idempotency key creates duplicate candidates
different payload for the same idempotency key is accepted
SQLite types leak into core, ports, or application public APIs
any lint, compile, doc, or test warning appears
```
