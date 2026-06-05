# Master Plan Gap Assessment - 2026-06-05

## Status

The current repository is an internal prototype, not a complete Idiolect v1 implementation.

The previous "complete" status was inaccurate. The codebase has useful Rust-first scaffolding, contract tests, fixture flows, and package smoke checks, but it does not satisfy the master plan's v1 delivery target in `docs/idiolect_master_plan_rust_first.md`.

Assessment inputs:

- Local repo inspection on 2026-06-05.
- Read-only `gpt-5.3-codex-spark` subagent assessments for daemon/packaging, ML/training, storage/privacy/audio, and gates.
- Direct binary checks:
  - `target/release/idiolectd --version --json` exits 0.
  - `target/release/idiolectd` exits 2 with `command is required`.
  - `target/release/idiolectd run` exits 2 with `unknown command: run`.

## Executive Summary

Major gaps:

- No real `idiolectd run` daemon.
- No config loader or XDG runtime/data/cache layout.
- No live audio capture-to-ASR daemon loop.
- No real Fcitx5 install metadata or desktop input-method integration.
- No Burn/Candle/Rust-native trainer backend.
- No actual training artifact generation.
- No persistent adapter registry state machine matching the master plan.
- No complete storage schema for users, utterances, audio files, app context, retention, and derived adapters.
- Privacy delete/export cover database rows only, not audio/cache/artifacts/manifests.
- CLI product command surface is mostly absent.
- Packaging validates payload extraction only, not install/enable/disable/upgrade/uninstall.
- Test gates pass but do not prove the master-plan all-done acceptance gates.

## Implemented Foundation

The following parts are real and useful:

| Area | Current evidence |
| --- | --- |
| Rust workspace and lint gates | `Cargo.toml`, `ci/scripts/test-rust.sh` |
| Boundary crates | `idiolect-core`, `idiolect-ports`, adapter crates |
| IPC basics | `crates/idiolect-ipc/src/{messages,framing,handshake}.rs` |
| Fixture dictation use case | `crates/idiolect-application/src/use_cases/dictation.rs` |
| SQLite event log baseline | `crates/idiolect-adapter-sqlite/migrations/0001_initial.sql` |
| Correction memory table baseline | `crates/idiolect-adapter-sqlite/migrations/0002_correction_memory.sql` |
| Fixture and real adapter contracts | CPAL, Opus, VAD, Whisper adapter tests |
| Minimal Fcitx5 C++ client | `fcitx5/idiolect-fcitx5/src` |
| Minimal package build | `ci/scripts/test-packaging.sh` |
| Coverage/quality scripts | `ci/scripts/test-all.sh`, `test-coverage*.sh` |

## Gap Matrix

| Master area | Required by master plan | Current state | Gap |
| --- | --- | --- | --- |
| Product goal | Complete dictation-to-learning loop | Fixture flows and partial integration tests | No complete local product loop |
| Binary names | `idiolectd`, `idiolect`, optional `idiolect-train` | `idiolectd`, `idiolect-cli`, `idiolect-trainerctl` | User-facing binary surface does not match plan |
| `idiolectd run` | Main daemon command | Missing; exits `unknown command: run` | No service daemon |
| Daemon responsibilities | Config, IPC socket, audio capture, VAD, ASR, codec/storage, trainer jobs | Fixture commands only | Runtime composition root incomplete |
| Configuration | `~/.config/idiolect/config.toml` with user/audio/vad/asr/storage/training sections | `crates/idiolect-common/src/config.rs` is placeholder | No config schema/parser/validation |
| XDG file layout | data/config/cache/model/audio paths | Temp paths and CLI args | No production file layout |
| Audio pipeline | Capture, buffering, resampling, VAD, codec, audio store | Fixture pipeline and adapter contracts | No live streaming pipeline in daemon |
| ASR model plan | Model profiles, medium/en target, GPU/thread options | Tiny fixture model path only | No model manager/profile matrix |
| Fcitx5 engine | Real input-method addon, metadata, recovery, app behavior | Thin C++ engine/client and tests | No registered input-method package or desktop matrix |
| Storage schema | users, utterances, ime sessions, edit events, candidates, adapters, runs | sessions/events/candidates/adapters/runs only, no users/utterances/audio metadata | Schema does not match master plan |
| Audio file store | Ogg Opus files, decoded cache, retention | No persistent audio store pathing | Audio lifecycle absent |
| Privacy | Delete audio/text/edit/candidates/cache/manifests; strict deleted-sample exclusion | DB row deletion and manifest exclusion flag | No audio/cache/artifact deletion; no strict adapter derivation handling |
| Correction memory | User-scoped repeated-term learning with lifecycle/delete | Minimal raw/corrected table | Not integrated into ASR draft improvement or delete controls |
| Trainer | Rust trainer can consume manifests and emit adapter artifacts | `TrainerPort` trait only; `trainerctl` binary prints crate name | No trainer implementation |
| Burn/Candle | Burn preferred Rust training candidate, Candle alternative | No Burn/Candle deps or crates | Missing ML backend path |
| Evaluation | WER/CER/proper noun/command/hallucination/deletion/latency/RTF | Small synthetic metric delta struct | Incomplete metric and evaluation pipeline |
| Adapter registry | Persistent current/previous/best/historical states | In-memory single rollback helper | No persistent atomic registry |
| CLI | doctor/service/models/sessions/memory/candidates/train/adapters/privacy | doctor and privacy only | Product command surface absent |
| Packaging | daemon, CLI, Fcitx5 metadata, config templates, systemd user service, install lifecycle | `.deb` with two binaries, `.so`, doc README | Not install-ready |
| Test strategy | unit/integration/contract/E2E/privacy/model/perf/package/app matrix | Many tests, but fixture-heavy | Gates do not prove all-done criteria |

## Misleading Or Weak Evidence

- `docs/superpowers/plans/2026-06-04-idiolect-v1-rust-first-implementation.md` says child plans 05-07 are complete. That is too broad relative to the master plan.
- `docs/quality/v1-coverage-map.md` maps multiple learning rows to one synthetic test and package rows to scripts, not behavioral test IDs.
- `ci/scripts/test-coverage-map.sh` only checks row names and `UNASSIGNED`; it does not prove mapped tests exist or cover behavior.
- `ci/scripts/test-all.sh` does not run `ci/scripts/test-package-smoke.sh`, while `README.md` says it does.
- `crates/idiolect-cli/src/lib.rs` returns a hardcoded doctor result.
- `serve-real-fixture` checks audio/model paths but transcribes an in-memory fixture and loads the fixture model through `WhisperAsr::load_fixture_model()`.
- Learning tests use handcrafted reports and compatibility values; no trainer produces an artifact.

## Subagent Assessment Summary

`gpt-5.3-codex-spark` read-only assessors found:

- Daemon/packaging: no real `idiolectd run`, no continuous daemon, no config, no install metadata or service lifecycle.
- ML/training: no Burn/Candle backend, no artifact generation, no runtime trainer/evaluator wiring.
- Storage/privacy/audio: schema lacks users/utterances/audio metadata and lifecycle; delete is DB-only.
- Gates: current gates are useful quality checks but not all-done acceptance gates.

## Recovery Priority

1. Correct the release status and harden gates so they cannot imply v1 completeness.
2. Implement config, XDG layout, and real `idiolectd run`.
3. Complete storage/audio/privacy schema and retention lifecycle.
4. Wire live audio, VAD, ASR, codec, storage, IPC, and Fcitx5 into a product flow.
5. Implement trainer/evaluator/artifact/adapter registry, including Burn as the first Rust-native backend candidate behind a port.
6. Expand packaging and desktop integration.
7. Add all-done E2E, failure, privacy, model, performance, and packaging gates.

## Decision

The repo should be treated as a Rust-first prototype baseline. It should not be described as Idiolect v1 complete until every gate in `docs/idiolect_master_plan_rust_first.md` sections 26.4, 26.5, and 29.17 has executable evidence.
