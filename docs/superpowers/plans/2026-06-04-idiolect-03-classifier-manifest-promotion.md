# Idiolect 03 Classifier Manifest Promotion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Rust-native learning control layer: candidate classification, deterministic manifest generation, metric report parsing, promotion decisions, and rollback state.

**Architecture:** `idiolect-trainerctl` owns learning-control logic and uses only Idiolect-owned domain types plus pinned Rust crates. Python remains research-only and cannot be part of classification, manifest, promotion, rollback, or required tests.

**Tech Stack:** Rust, Serde JSON, exact pinned digest crate selected by decision record, strict Cargo lint gates, deterministic fixtures in Rust tests.

---

## Scope Boundary

Allowed behavior:

```text
candidate classification rules
manifest construction
manifest digesting
metric report DTOs
artifact compatibility checks
promotion and rollback policy
trainerctl unit and integration tests
```

Forbidden behavior:

```text
Python required-path trainer or evaluator
real model training
GPU or accelerator code
storage schema changes
real ASR/VAD/audio adapters
Fcitx5 shim changes
```

Required gates after every code task:

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

## Metric Delta Convention

Every metric delta is `candidate_adapter_metric - active_adapter_metric`.

```text
personal_wer_delta < 0.0 means personal WER improved
general_wer_delta > 0.0 means general WER regressed
hallucination_delta > 0.0 means hallucination rate regressed
p95_latency_delta_ms > 0 means latency regressed
proper_noun_accuracy_delta > 0.0 means proper-noun accuracy improved
```

Default promotion policy rejects any positive general WER delta, any positive hallucination delta, any positive p95 latency delta, and any personal WER delta greater than `-0.01`.

## Task 1: Candidate Classifier

**Owner:** Spark worker allowed, gatekeeper reviews classification thresholds  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-trainerctl/src/lib.rs`
- Create: `crates/idiolect-trainerctl/src/classifier.rs`
- Modify: `crates/idiolect-trainerctl/Cargo.toml`

- [ ] **Step 1: Write failing classifier tests**

Create tests for:

```text
preedit_correction_is_approved_high_value_evidence
accepted_without_edit_is_observed_but_lower_trust
unchanged_preedit_correction_is_rejected
empty_accepted_text_is_rejected
```

The expected labels are:

```rust
CandidateLabel::Approved { trust_score_bps: 10_000 }
CandidateLabel::Approved { trust_score_bps: 6_000 }
CandidateLabel::Rejected { reason: "unchanged_text" }
CandidateLabel::Rejected { reason: "empty_text" }
```

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-trainerctl --lib classifier
```

Expected: FAIL because classifier types are absent.

- [ ] **Step 3: Implement classifier**

Implement:

```rust
pub enum CandidateEvidence {
    PreeditCorrection { raw_text: String, corrected_text: String },
    AcceptedWithoutEdit { text: String },
}

pub enum CandidateLabel {
    Approved { trust_score_bps: u16 },
    Rejected { reason: &'static str },
}

pub struct CandidateClassifier;
```

Rules:

```text
PreeditCorrection with changed trimmed text: Approved 10000 bps
PreeditCorrection with identical trimmed text: Rejected unchanged_text
AcceptedWithoutEdit with non-empty trimmed text: Approved 6000 bps
AcceptedWithoutEdit with empty trimmed text: Rejected empty_text
```

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-trainerctl --lib classifier
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolect-trainerctl
git commit -m "feat: add rust candidate classifier"
```

## Task 2: Deterministic Manifest Builder

**Owner:** Spark worker allowed  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Create: `docs/decisions/0003-learning-manifest-digest.md`
- Modify: `Cargo.toml`
- Modify: `crates/idiolect-trainerctl/Cargo.toml`
- Create: `crates/idiolect-trainerctl/src/manifest.rs`
- Modify: `crates/idiolect-trainerctl/src/lib.rs`

- [ ] **Step 1: Record digest dependency decision**

Use the already verified pinned workspace dependency `sha2 = "=0.11.0"` selected on 2026-06-04. Create `docs/decisions/0003-learning-manifest-digest.md` with exact crate version, digest algorithm `SHA-256`, and the reason manifest digesting is deterministic and local-only. Record that this decision authorizes trainerctl to use `sha2` privately for learning manifests and supersedes child-02-only wording for `sha2`; `rusqlite` remains confined to the SQLite adapter.

- [ ] **Step 2: Write failing manifest tests**

Create tests `manifest_includes_only_approved_candidates_in_stable_order` and `manifest_digest_is_stable_for_same_inputs`. The first must push approved candidate `b`, rejected candidate `c`, approved candidate `a`, then assert the built manifest contains `a` followed by `b`. The second builds the same manifest twice and asserts equal 64-character lowercase hex digests.

- [ ] **Step 3: Run red command**

```bash
cargo test -p idiolect-trainerctl --lib manifest
```

Expected: FAIL because manifest builder types are absent.

- [ ] **Step 4: Implement manifest builder**

Required behavior:

```text
reject empty user_id
include only approved candidates
sort approved candidates by id ascending
serialize manifest input with serde_json after sorting
digest serialized bytes with SHA-256 lowercase hex
store digest on Manifest
```

- [ ] **Step 5: Run green command and gates**

```bash
cargo test -p idiolect-trainerctl --lib manifest
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add docs/decisions/0003-learning-manifest-digest.md Cargo.toml crates/idiolect-trainerctl
git commit -m "feat: add deterministic training manifest builder"
```

## Task 3: Metric Reports And Artifact Compatibility

**Owner:** Spark worker allowed, gatekeeper reviews compatibility semantics  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Create: `crates/idiolect-trainerctl/src/metrics.rs`
- Modify: `crates/idiolect-trainerctl/src/lib.rs`

- [ ] **Step 1: Write failing metrics tests**

Create tests `metric_report_round_trips_and_preserves_delta_signs` and `artifact_compatibility_requires_base_model_runtime_and_digests`. The metric test must assert `personal_wer_delta == -0.12`, `general_wer_delta == 0.0`, and `p95_latency_delta_ms == -5` after JSON round trip.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-trainerctl --lib metrics
```

Expected: FAIL because metric report and compatibility DTOs are absent.

- [ ] **Step 3: Implement DTOs**

Implement `EvaluationReport`, `MetricDeltas`, and `ArtifactCompatibility` with serde derives. `ArtifactCompatibility::is_compatible()` returns true only when artifact digest, manifest digest, metric report digest, base model id, adapter format version, runtime format version, and runtime-compatible flag are all valid.

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-trainerctl --lib metrics
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/idiolect-trainerctl/src/metrics.rs crates/idiolect-trainerctl/src/lib.rs
git commit -m "feat: add learning metric and compatibility dto types"
```

## Task 4: Promotion Decision Policy

**Owner:** Gatekeeper-local or stronger model for policy; Spark worker can implement approved rules  
**Model:** Gatekeeper-local for final policy acceptance; `gpt-5.3-codex-spark` for implementation after rules are accepted  
**Files:**

- Create: `crates/idiolect-trainerctl/src/promotion.rs`
- Modify: `crates/idiolect-trainerctl/src/lib.rs`
- Modify: `crates/idiolect-trainerctl/src/metrics.rs` only if a constructor or test fixture helper is required for policy tests

- [ ] **Step 1: Write failing promotion tests**

Create tests:

```text
promote_when_personal_improves_and_general_quality_does_not_regress
reject_when_general_wer_regresses
reject_when_artifact_is_not_runtime_compatible
reject_when_personal_wer_does_not_improve_enough
reject_when_latency_regresses
```

Use the delta convention in this plan. General WER regression is `general_wer_delta = 0.01`. Personal improvement that qualifies is `personal_wer_delta = -0.08`.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-trainerctl --lib promotion
```

Expected: FAIL because promotion policy is absent.

- [ ] **Step 3: Implement promotion policy**

Implement:

```rust
pub enum PromotionDecision {
    Promote,
    Reject { reason: &'static str },
}

pub struct PromotionPolicy {
    pub max_general_wer_delta: f32,
    pub max_hallucination_delta: f32,
    pub max_p95_latency_delta_ms: i32,
    pub min_personal_wer_improvement: f32,
}
```

Default values:

```text
max_general_wer_delta = 0.0
max_hallucination_delta = 0.0
max_p95_latency_delta_ms = 0
min_personal_wer_improvement = -0.01
```

Stable reject reasons:

```text
artifact_incompatible
personal_wer_not_improved
general_wer_regression
hallucination_regression
latency_regression
proper_noun_accuracy_regression
```

- [ ] **Step 4: Run green command and gates**

```bash
cargo test -p idiolect-trainerctl --lib promotion
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/plans/2026-06-04-idiolect-03-classifier-manifest-promotion.md crates/idiolect-trainerctl/src/promotion.rs crates/idiolect-trainerctl/src/lib.rs crates/idiolect-trainerctl/src/metrics.rs
git commit -m "feat: add adapter promotion policy gates"
```

## Task 5: Rollback State And Learning Integration Test

**Owner:** Spark worker allowed, gatekeeper reviews rollback semantics  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-trainerctl/src/promotion.rs`
- Create: `crates/idiolect-integration-tests/tests/learning_promotion.rs`
- Modify: `crates/idiolect-integration-tests/Cargo.toml`
  - Add `idiolect-trainerctl = { path = "../idiolect-trainerctl" }` for the integration test only.

- [ ] **Step 1: Write failing rollback unit test**

Add `rollback_restores_previous_active_adapter`: register old active adapter, register new candidate, promote new candidate, rollback user `default`, then assert active adapter returns to old id.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-trainerctl --lib rollback_restores_previous_active_adapter
```

Expected: FAIL because rollback registry state is absent.

- [ ] **Step 3: Implement in-memory registry policy**

Implement `InMemoryAdapterRegistry` for policy tests:

```text
one active adapter per user
promote stores previous active adapter id in rollback slot
rollback swaps previous active adapter back into active slot
rollback with no previous adapter returns NoRollbackTarget
```

- [ ] **Step 4: Add integration test**

Create `learning_promotion.rs` asserting `evaluate_promotion(&PromotionPolicy::default(), &EvaluationReport::passing_for_test(), &ArtifactCompatibility::compatible_for_test()) == PromotionDecision::Promote`.

- [ ] **Step 5: Run green command and gates**

```bash
cargo test -p idiolect-trainerctl --lib
cargo test -p idiolect-integration-tests --test learning_promotion
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-trainerctl crates/idiolect-integration-tests
git commit -m "feat: add learning promotion rollback coverage"
```

## Rejection Criteria

Reject and rework this child if any condition holds:

```text
Python is required to classify, build manifests, evaluate promotion, or rollback
promotion accepts positive general WER delta under the default policy
promotion accepts positive hallucination delta under the default policy
promotion accepts positive latency delta under the default policy
promotion accepts artifact incompatibility
manifest digest changes when inputs are the same
candidate ordering depends on insertion order instead of stable id order
any digest dependency lacks an exact version in a decision record
any lint, compile, doc, or test warning appears
```

