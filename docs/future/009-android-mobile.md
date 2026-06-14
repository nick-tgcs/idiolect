# 009 — Android Mobile (idiolect-mobile)

**Status:** Future
**Priority:** High (new platform; large scope)
**Effort:** Large (multi-phase)

> Planning doc for an **Android-first** mobile Idiolect that dictates into other
> apps, captures corrections on-device, **ships those "learnings" to the PC** to
> limit phone storage, **trains there**, and pulls a personalised model back.
> It also records the decision on the user's open question — *new repo, or a
> mobile variant in this monorepo?*

---

## TL;DR

1. **Same monorepo.** Add Android as new crates inside this Cargo workspace plus
   a sibling `android/` Gradle project. Do **not** start a second repo. (Scored
   8.5 vs 7 vs 4.5 — see [Repo structure decision](#repo-structure-decision).)
2. **Reuse the brain, swap the edges.** `idiolect-core`, `idiolect-ports`,
   `idiolect-application`, `idiolect-common`, `idiolect-ipc`, `idiolect-ml-core`
   compile for `aarch64-linux-android` essentially unchanged. The portable
   adapters (`sqlite`, `opus`, `whisper`, `vad`, `crypto`) cross-compile under
   the NDK. Only the Linux-specific edges (IBus, cpal, ksni tray, arboard,
   x11rb, the four eframe GUIs) are replaced by Android-native pieces.
3. **The Android IME is just another `InputMethodPort`.** Today's
   `show_preedit / update_preedit / commit_text / cancel_preedit` maps **1:1**
   onto `InputConnection.setComposingText / commitText / finishComposingText`.
4. **whisper.cpp + ggml, not ONNX.** The trainer emits a merged whisper.cpp
   `.bin`, so on-device inference **must** be whisper.cpp/ggml (reuse
   `idiolect-adapter-whisper` with `cuda`/`vulkan` OFF) for the personalised
   model to load at all. CPU/NEON, 4 threads, VAD-gated full-utterance decode.
5. **Sync = tiny self-hosted push/pull over Tailscale.** Phone POSTs an
   encrypted batch (`ManifestV2Item`-shaped JSON + raw Opus + SHA-256), the PC
   ingests into the same SQLite + audio-root the trainer already reads, and the
   phone **deletes the local Opus only after a hash-confirmed ACK**. Training on
   the PC runs `trainerctl revalidate`/`train` **unchanged**.
6. **WhisperVault is a concept reference, not a code-lift** (it's GPL-3.0 and
   uses an ONNX engine). Borrow its recording pipeline, model-integrity SHA-256
   pattern, IME `commitText` mechanism, and mic-off-on-focus-loss privacy gate.

---

## Problem

Idiolect today is Linux-only: an IBus/Fcitx5 input method plus a daemon, with
GPU LoRA training that personalises the whisper model. The user wants the same
experience on Android **first**, but with two hard constraints:

- **Limit mobile storage.** A phone should not hoard gigabytes of training audio.
- **Train on the PC.** Keep the heavy Burn/CUDA LoRA pipeline where the GPU is;
  the phone is a capture + inference client, not a trainer.

The user has a tweaked fork, [`nick-tgcs/WhisperVault`](https://github.com/nick-tgcs/WhisperVault),
that is "similar but not quite the same" and whose concepts we can leverage.

## Goals / Non-goals

**Goals**
- Dictate into any Android app via a custom voice keyboard (IME).
- Capture raw→corrected pairs on-device exactly as the desktop does.
- Ship learnings to the PC, free phone storage after confirmed receipt.
- Train on the PC; optionally pull a personalised `.bin` back to the phone.
- Maximise Rust code reuse; keep one source of truth for the dictation pipeline.
- **Run on GrapheneOS** (de-Googled, hardened): no Google Play Services,
  network-optional, `hardened_malloc`-safe native libs, FOSS-only. See the
  *GrapheneOS compatibility* section in [009-android-implementation-plan.md](009-android-implementation-plan.md#grapheneos-compatibility--a-hard-target).

**Non-goals (initially)**
- On-device training (stays on PC).
- iOS (architecture keeps the door open; not in scope now).
- Real-time sliding-window streaming ASR (pathological on phones — see ASR).
- Translation on mobile (desktop translator is a subprocess; no Android analog yet).
- Shipping `medium`/`large` models to the phone (PC-only by size/RAM/thermals).

---

## What stays vs what's new

The codebase is a textbook hexagon: `idiolect-core` depends on **nothing**;
`idiolect-ports` holds the trait/DTO seams; `idiolect-application` orchestrates;
adapters point inward. Only `idiolectd` depends "up" on every adapter to compose
them. That structure is exactly what makes a second platform cheap.

| Layer | Crates | On Android |
|---|---|---|
| **Brain (pure Rust)** | `idiolect-core`, `idiolect-ports`, `idiolect-application`, `idiolect-common`, `idiolect-ipc`, `idiolect-ml-core` | **Reused unchanged** (one edit: an Android path provider in `idiolect-common/src/config.rs`, the only Linux-flavoured spot — `XdgBaseDirs` reads `HOME`/`XDG_*`). |
| **Portable edges (C/Rust)** | `idiolect-adapter-sqlite` (bundled), `idiolect-adapter-opus`, `idiolect-adapter-whisper` (`cuda` OFF), `idiolect-adapter-vad`, `idiolect-adapters/crypto` | **Reused via NDK cross-compile.** |
| **Desktop edges** | `idiolect-ibus`, `idiolect-adapter-cpal`, `idiolect-adapter-ksni`, `idiolect-adapter-clipboard` (arboard), x11rb focus, the four `eframe` GUIs, `desktop_integration.rs`, `settings_launcher.rs` | **Dropped / replaced** by Android-native equivalents. |
| **Host-only tooling** | `idiolect-trainer-burn`, `idiolect-trainerctl`, `idiolect-cli`, fixtures, integration tests | **Never on device.** Training stays on the PC. |

### Port → Android mapping

| Port (`idiolect-ports`) | Desktop adapter | Android replacement |
|---|---|---|
| `InputMethodPort` (`input_method.rs`) | IBus engine (`idiolect-ibus`) | Kotlin `InputMethodService` over a UniFFI callback → `InputConnection` |
| `AudioInputPort` (`audio.rs`) | `idiolect-adapter-cpal` | `AudioRecord`/AAudio (Kotlin) pushing PCM over JNI |
| `AsrPort` (`asr.rs`) | `idiolect-adapter-whisper` | **Same crate**, `cuda`/`vulkan` OFF, CPU/NEON |
| codec / vad / storage / crypto | opus / vad / sqlite / crypto | **Same crates**, NDK-built |
| tray, clipboard, GUIs | ksni / arboard / eframe | Android Notification + `ClipboardManager` + Compose Activity |

The desktop's separate-process **Unix-socket daemon collapses to in-process
method calls** on Android: the `idiolect-ipc` message enum survives as the
internal command/event vocabulary, delivered as UniFFI calls + a callback flow
instead of newline-JSON over a socket. The X11 focus-juggling
(`_NET_ACTIVE_WINDOW`, focus-settle) is **deleted, not ported** — the IME owns
its `InputConnection`.

---

## Android runtime architecture

A single Android app = **Kotlin/Compose edges + one in-process Rust core
(`.so`)**. The product surface *is* the keyboard, so everything orbits an
`InputMethodService` backed by a microphone foreground service.

Components:

1. **`IdiolectImeService`** (Kotlin, `extends InputMethodService`) — the voice
   keyboard, the analog of the desktop IBus engine. Minimal Compose input view:
   a mic key + an inline correction strip (honours the **one-surface** rule —
   no new windows). Owns the `InputConnection`; implements the UniFFI
   `InputMethod` callback (`setComposingText` for partials, `commitText` for the
   finalised take, `deleteSurroundingText`+`commitText` for corrections). It is
   the **only legal mic trigger**: the active input method is exempt from *both*
   the Android 12+ background-FGS-start restriction *and* the Android 14
   while-in-use mic restriction. Privacy gate: stop capture on
   `onFinishInputView` / `onWindowHidden` / `inputType == TYPE_NULL`.
2. **`MicForegroundService`** (Kotlin) — `foregroundServiceType="microphone"` +
   `FOREGROUND_SERVICE_MICROPHONE` + `RECORD_AUDIO`. Runs `AudioRecord` at 16 kHz
   mono on a dedicated capture thread; pushes frames into Rust over JNI. Survives
   the user navigating to the target app mid-take.
3. **On-device whisper** (`idiolect-adapter-whisper`, unchanged logic) — see ASR.
4. **Local encrypted storage** (`idiolect-adapter-sqlite` + `crypto`, unchanged
   schema) — SQLite + IDOPUS1 Opus files under app-private `filesDir`. The crypto
   key moves from a `0600` `FileKey` file to the **Android Keystore** (unix perms
   are meaningless in the sandbox).
5. **Correction/review UI** (Compose) — inline strip in the IME view is primary;
   a richer history/review screen is a separate Activity (the IME view is too
   small and can't show runtime-permission dialogs).
6. **Model management** (Kotlin UI + Rust verify) — authenticated base-model
   download with progress/resume (not WhisperVault's browser-punt), SHA-256
   integrity verified at extract **and** every load, Zip-Slip hardening.
7. **Sync client** (Rust, scheduled by Kotlin `WorkManager`) — see sync protocol.

### Boundary & threading

- **One binding mechanism: UniFFI** (`#[uniffi::export]`), hand-JNI only for the
  hot PCM frame push. UniFFI is preferred specifically because the workspace
  lints set `unsafe_code = "forbid"` — raw JNI is `unsafe` and would violate it.
- Kotlin→Rust: `toggle()`, `commit()`, `cancel()`, `report_correction()`,
  `history_edited()`, `push_pcm_frame()`, `download/verify_model`, `sync_now()`.
- Rust→Kotlin (callback flow, the `idiolect-ipc` variants): `RecordingStatus`
  (authoritative, edge-triggered — the UI never toggles its mic indicator
  optimistically; it waits for this push, preserving the desktop single-source-
  of-truth invariant), `PreeditUpdate`, `InsertText`, `EditHistory`.
- Threads: (a) mic capture (Kotlin `AudioRecord`) → JNI → Rust buffer;
  (b) Rust decode worker on a ~150 ms tick: drain → resample16k → `FrameBuffer(480)`
  → VAD → `UtteranceSegmenter`, per-snippet decode emits partials, **stop-time
  full re-decode of the whole take is authoritative**; (c) Kotlin UI thread
  applies `setComposingText`/`commitText`; (d) `WorkManager` for maintenance +
  sync flush.

> ⚠️ **Carry the full-take re-transcribe-at-stop policy to Android or the
> "streaming drops words" bug resurfaces** and poisons the training pairs shipped
> to the PC. This is why the orchestration must be *shared*, not re-implemented
> (see Phase 2).

---

## On-device ASR decision

**Reuse `idiolect-adapter-whisper` (whisper-rs 0.16 → whisper.cpp 1.8.3), CPU/NEON,
`cuda`/`vulkan` OFF.** GPU is already gated purely behind the `cuda` cargo feature
(`GPU_ENABLED = cfg!(feature = "cuda")`), so an Android build that omits it runs
CPU with **zero logic change**. The crate already does full-segment beam-search
decode with `no_timestamps` — exactly the VAD-then-full-decode pattern Android
needs — and the `tokenize`/`detokenize` seam yields training pairs identical to
desktop.

**Models (quantised Q5_1 to address the storage concern):**

| Model | Size | Role |
|---|---|---|
| `tiny.en` Q5_1 | ~31 MB | low-RAM / older-device fallback |
| `base.en` Q5_1 | ~57 MB | **default** (near-real-time on flagships, RTF ~0.4) |
| `small.en` Q5_1 | ~182 MB | opt-in "high accuracy" |
| `medium` Q5_0 | ~515 MB | **PC-only** (RAM/thermals) — never shipped to phone |

**Acceleration reality (2026):** CPU NEON @ 4 threads is the only universally
reliable backend. Vulkan is **not NDK-buildable** (NDK ships no
`<vulkan/vulkan.hpp>`); OpenCL/Adreno crashes on whisper.cpp; **NNAPI is
deprecated in Android 15**. Qualcomm QNN/NPU is genuinely real-time but a separate
per-SoC ONNX stack — treat as a *future, vendor-specific* lane, not the baseline.

**Streaming:** do **not** do sliding-window streaming (5–7× slower than real-time
on Android, latency compounding). Synthesize the "live" feel from per-snippet
decode + a final full-take decode, exactly as desktop already does.

---

## Rust-on-Android stack decision

**Native Android (Kotlin + Jetpack Compose) + Rust core via UniFFI, built with
cargo-ndk.** (Rejected: Flutter — adds a 3rd language for only a richer settings
UI; Tauri — a WebView **cannot be an IME**, so it's dead weight for the core
surface; Slint/egui-on-Android — self-described not-production-ready and *still*
need the Kotlin IME.)

The decisive constraint: **an Android IME must be a Kotlin/Java
`InputMethodService` with a native `View`/Compose input surface** — no UI
framework "provides an IME for free." So every stack converges on the same hard
work (Kotlin IME Service + microphone FGS calling the shared Rust core). Given
that, a non-native UI framework buys nothing for the core surface and adds a
second binding layer. UniFFI is Mozilla-grade (Firefox/AOSP); cargo-ndk 4.x is
stable; Compose-in-IME is a known pattern; RustDesk proves the "framework UI +
Rust core + extra hand-written Android Service" shape ships in production.

**Cross-compile cost (the real porting work, same for any stack):**
`rustup target add aarch64-linux-android x86_64-linux-android …`; NDK r26/r27;
`cargo ndk -t arm64-v8a -t x86_64 -o android/app/src/main/jniLibs build --release`.
cargo-ndk supplies the NDK clang so `libsqlite3-sys` (bundled), opus, and
whisper.cpp's CMake all link. Gotchas: bundle `libc++_shared`, disable LTO, maybe
copy `libunwind.a`. Target **arm64-v8a primary**, x86_64 for the emulator.

---

## Learning-sync protocol (the crux)

> Goal: ship learnings to the PC, train there, free phone storage, optionally
> ship a model back. This is the heart of the user's request.

### What "a learning" is

One learning = one `training_candidates` row + its utterance's Opus audio +
transcripts — exactly the trainer's input contract. The export unit is shaped
like the existing **`ManifestV2Item`** (`idiolect-trainerctl/src/manifest.rs`):
`user_id, training_candidate_id, utterance_id, text_session_id, audio_object_key,
audio_digest, raw_transcript, corrected_transcript, source_label, trust_score_bps,
base_model_id`. **Audio is required** to train (Burn LoRA trains on audio+text),
which is precisely why "limit mobile storage" means *ship then delete the Opus*.

> **Graft from the "extract shared core" approach:** put the wire types in a new
> shared crate **`idiolect-sync`**, consumed by *both* the phone and the PC.
> `ManifestV2Item` itself can't be reused on the wire — its fields are private and
> it has no `Deserialize` — so define a dedicated `SyncLearning` DTO
> (`Serialize + Deserialize`) with the same field names. **Deliberate divergence:**
> the phone must **not** set `split` (train/val/holdout) — that's a corpus-level
> decision the PC owns at train time.

### Wire format

- `POST /v1/learnings/batch` (phone → PC, the hot path): a **length-prefixed
  binary container** (`Content-Type: application/vnd.idiolect.sync.v1`,
  magic `IDSYNC1`) so audio stays binary (no base64 bloat) **and** without the
  boundary-collision risk of `multipart/mixed` over binary blobs. Layout: JSON
  `{device_id, batch_id, learnings:[SyncLearning]}` then `audio_count` parts of
  raw IDOPUS1 bytes, each **content-addressed by its `audio_digest`** (so a
  digest shared by two learnings ships its bytes once). Response:
  `{accepted, rejected, already_have}`. **Idempotent** on `(device_id, audio_digest)`.
  Implemented in [`idiolect-sync`](../../crates/idiolect-sync/src/codec.rs).
- `GET /v1/model/current?base=<id>&since=<version>` (PC → phone): the merged ggml
  `.bin` as `octet-stream`, `Range`-resumable, `X-Artifact-Sha256` verified; `304`
  if current.
- `GET /v1/health` — reachability probe for the outbox pump.

`audio_digest` = SHA-256 of the Opus payload, computed at export. **Today
production never populates `utterances.audio_sha256`** (only a test helper does) —
the capturer must add the hash step. It is both the content-addressed dedup key
and the ACK token.

### Transport, auth, encryption

- **Self-hosted HTTPS push/pull over Tailscale/WireGuard** (no public ports, NAT
  traversal, works on cellular). Self-host **Headscale** if zero third-party
  control plane is wanted. (Rejected: Syncthing — send-only *propagates* the
  phone's deletion to the PC, receive-only strands the phone; neither does
  upload-then-free-local-space. WebDAV — you still hand-build outbox/retry/
  versioning, so it converges to this anyway.)
- **Pairing:** PC shows a short-lived QR/code (settings UI); phone scans it,
  handshake (reuse the `idiolect-ipc` `ClientHello`/`ServerHello` concept) mints a
  per-device bearer token bound to `device_id`, stored in the Android Keystore.
  Server maps token → `user_id` (so `user_id` is verified server-side).
- **Encryption:** TLS in transit over the tailnet. At rest on the phone, wrap the
  on-disk outbox with the existing `ChaCha20Poly1305` crypto adapter under a
  Keystore key. Note the desktop history `FileKey` is per-device random and won't
  decrypt on the PC — at-rest encryption is phone-local only; the wire payload is
  TLS-protected.

### Delete-after-confirmed-shipped (reclaim storage)

Safe **only because audio is training-only today** (nothing in the serving path
re-reads source audio). Sequence: export batch → POST → on `2xx` ACK, flip each
acked candidate to a **new additive `synced` status** on `training_candidates`
(the column already exists; adding a value needs no migration), then delete
**only** the Opus file via the existing narrow
`delete_source_audio_for(user_id, utterance_id)` — **never** the coarse,
time-based `prune_training_data` (which also drops rows/sessions). The row +
transcript stay for provenance. **Never prune an un-synced candidate.**

### PC-side ingest

`idiolectd` grows an HTTP listener bound to the tailnet (or a separate
`idiolect-sync-server` binary — see open questions). The handler verifies the
token, verifies `SHA-256(audio) == audio_digest`, dedups on `(device_id, digest)`,
then writes the Opus via `FileAudioStore.write_source_audio` into the trainer's
audio-root and INSERTs `training_candidates` + `utterances`
(`audio_sha256 = digest`) + `ime_text_sessions` so the trainer's existing JOIN is
satisfied. After ingest the PC runs the **unchanged** pipeline:
`trainerctl revalidate --apply` then `trainerctl train --base-model … --output …`,
producing an ordinary merged ggml `.bin`. The phone pulls a tiny/base-sized
personalised `.bin` and atomically swaps the on-device model (which loads the
merged `.bin` unchanged — no inference-side adapter support needed).

---

## WhisperVault — borrow vs avoid

WhisperVault is an offline Android voice IME (Java/Gradle, `minSdk 28`), a
security-hardened fork of `woheller69/whisperIMEplus` (RTranslators lineage),
**GPL-3.0**. Its ASR is Whisper on **Microsoft ONNX Runtime** (6-file split
model), **not whisper.cpp**, with **no JNI/NDK**.

**Borrow (concepts, not code):**
- The **recording pipeline shape** — `AudioRecord` 16k mono + WebRTC VAD +
  pre-roll ring buffer + utterance queue + three modes (manual / one-shot /
  continuous). Matches Idiolect's existing VAD work.
- The **SHA-256 model-integrity pattern** (pin per-file hashes, verify at extract
  **and** every load, user-facing mismatch dialog) and **Zip-Slip** hardening —
  adopt wholesale.
- The **IME `InputConnection.commitText` mechanism** and **stop-mic-on-focus-loss**
  privacy gate.

**Avoid:**
- **The source verbatim** — GPL-3.0 would force Idiolect-mobile to GPL (see
  Licensing).
- **The ONNX 6-file engine** — Idiolect's trainer emits a whisper.cpp ggml `.bin`,
  so the personalised-model round-trip *requires* whisper.cpp/ggml on the phone.
  An ONNX engine couldn't load the trained model at all.
- **The browser-punt model download** (build a proper in-app download).
- **Per-utterance decode in continuous mode** — reproduces the "streaming drops
  words" bug. Keep full-take re-transcribe-at-stop.
- **The three-surface + on-keyboard mode-button UI sprawl** — conflicts with the
  one-surface rule.

---

## Repo structure decision

**Decision: keep it in this monorepo (Approach A).** Scored against the
alternatives by an adversarial judge:

| Approach | Score | Verdict |
|---|---|---|
| **A — Mobile in this monorepo** (android adapter subtree + UniFFI facade + sibling Gradle) | **8.5** | **Chosen.** Max reuse via path deps, zero version-skew, one git history/PR flow/branch-protection ruleset. Reuses the existing `crates/idiolect-adapters/{desktop/*,crypto}` platform-nesting precedent. |
| C — Extract shared engine, thin shells | 7 | Same end-state as A but front-loads a behaviour-neutral `git-mv` of ~25 crates (rewrite every path dep + `members`) while keeping TDD/clippy/coverage gates green — pure risk/time for hygiene that can be deferred. |
| B — Dedicated mobile repo consuming the brain as pinned git deps | 4.5 | Recurring tax: cross-repo `[workspace.dependencies]` must mirror pins exactly (`serde =1.0.228`, `rusqlite =0.40.0`, `whisper-rs =0.16.0`) or break; every coordinated change becomes a two-repo tag-and-bump; doubles the surface where the load-bearing full-take re-transcribe logic can drift. |

Key facts that make A the clear minimal-waste choice:

- **The host gate is safe.** `Cargo.toml` `members` is an *explicit* list (no
  `default-members`, no glob, no `exclude`), so the Android-only crates simply
  aren't listed — `cargo test --workspace` compiles exactly what it does today.
- **The monorepo is already polyglot.** CI already builds a C++/CMake `fcitx5`
  job, so a cargo-ndk lane is incremental, not categorical. (This also neutralises
  B's only selling point — toolchain isolation is free in A by not listing the
  FFI crate.)
- **Anti-drift.** The streaming orchestration must be lifted out of
  `idiolectd/run_loop.rs` regardless (all approaches need it). A path-dep makes
  accidental divergence *structurally impossible*; B's cross-repo boundary makes
  divergence the default failure mode — and divergence here resurrects the
  training-pair-poisoning bug.

**Grafts adopted from B/C:**
- Shared **`idiolect-sync`** wire-types crate (consumed by phone *and* PC).
- A **`cargo tree` CI assertion** that the FFI facade's graph contains none of
  `{burn, burn-cuda, ibus, ksni, arboard, x11rb, eframe, cpal, idiolect-trainer-burn,
  idiolect-trainerctl}` — buys C's "no Linux/CUDA crate can leak into the phone
  build" guarantee without the 25-crate move.
- A **`PathProvider` trait** rather than a hand-injected `XdgBaseDirs` override.
- A **versioned model/protocol** endpoint even though it's one repo.
- Defer C's directory zoning until crate count justifies it.

### Concrete layout

```
crates/idiolect-adapters/android/        # mirrors the existing desktop/ subtree
  capture/   idiolect-adapter-android-audio   # AudioInputPort via AudioRecord
  ime/       idiolect-adapter-android-ime     # InputMethodPort over an FFI callback
crates/idiolect-sync/          # SHARED wire types (SyncLearning, SyncBatch, binary container codec) ✅ exists
crates/idiolect-sync-client/   # ✅ logic: build_batch (outbox→envelope) + confirm_shipped (ACK→reclaim)
crates/idiolect-sync-server/   # ✅ ingest logic (envelope→rows+audio, idempotent); HTTP/GET-model still TODO
crates/idiolect-mobile-runtime/  # Android twin of idiolectd's run_loop (in-process, no socket)
crates/idiolect-ffi/           # the ONE UniFFI facade; the only cdylib/.so; kept OUT of `members`

android/                       # Gradle project, sibling to crates/ (NOT a crate)
  app/    Kotlin: InputMethodService, mic FGS, Compose settings/history Activity, manifest
  build.gradle  -> cargo-ndk + uniffi-bindgen, consumes idiolect-ffi as an AAR
  jniLibs/      per-ABI .so (arm64-v8a primary; x86_64 emulator)
```

Android crates are **kept out of `members` and `cfg`-gated to
`target_os = "android"`**, so the x86_64 host `--workspace` gate stays green;
a separate `aarch64-linux-android` cargo-ndk CI job gates the device build.

---

## Phased plan (TDD throughout)

Ordered so each phase lands green on its own and the **desktop product benefits
first**. The repo is strictly TDD — every phase is red→green with unit +
integration coverage; the Kotlin `onCreateInputView` is the one genuine GUI
boundary marked untestable-headless (with the reason stated), mirroring the
existing IBus/eframe caveats.

**Sync-protocol track (mostly desktop-side, no Android needed):**
- **S0 — Foundation.** ✅ **Done.** New `idiolect-sync` crate: `SyncLearning`/`SyncBatch`
  DTOs + length-prefixed binary container codec (unit-tested round-trip + framing
  errors). SHA-256 `audio_digest` compute lives in `idiolect_common::digest`, and
  capture now **populates `utterances.audio_sha256` (desktop too)** via
  `persist_session` → `set_audio_digest` — this also unblocks manifest validation,
  which rejects empty digests.
- **S1 — Delete-after-ship locally.** ✅ **Done.** Added the `synced` status +
  `mark_synced_and_drop_audio` (status flip then narrow `delete_source_audio_for`)
  + the outbox query `training_candidates_pending_sync` (`status = 'captured'`);
  the manifest feed now also excludes `synced`. Proven on desktop: audio dropped
  while row+transcript survive and the remaining captured candidate still trains.
- **S2 — Transport + ingest on one box.** ✅ **Ingest logic + one-box round-trip
  done in-process** (`idiolect-sync-server::ingest`, content-addressed idempotent;
  `sync_round_trip.rs`: build → wire codec → ingest into a second data-root →
  trainable candidates with audio intact → reclaim). **Remaining:** the HTTP hop
  (axum `POST` beside the Unix listener) + an `idiolect-cli` push subcommand, and
  extending the e2e through `trainerctl train` on box B to a merged `.bin`.
- **S3 — Auth + pairing.** QR/code handshake → per-device bearer token,
  idempotency (`batch_id` + `(device_id,digest)` dedup), at-rest outbox encryption.

**Mobile track:**
- **M0 — Build plumbing (the real cost).** ✅ **Cross-compile proven** (NDK r28 +
  cargo-ndk): the brain + portable adapters build for `aarch64-linux-android` and
  `x86_64-linux-android` via `scripts/android-cross-build.sh` (whisper.cpp, opus,
  bundled SQLite, webrtc-vad all clean; `cuda`/`vulkan` OFF). A dead
  `usearch`/`numkong` C++ dep was removed. **Still TODO:** `libc++_shared`
  bundling + LTO-off release; and **verify the Android whisper-rs/ggml build
  against the same fixture as the desktop parity test** (run on the emulator) so
  the tokenizer/decode doesn't drift.
- **M1 — Path provider + UniFFI facade.** `PathProvider` trait (XDG desktop impl,
  `filesDir` Android impl); `idiolect-ffi` exposing `toggle/commit/cancel/
  report_correction` + the `RecordingStatus`/`PreeditUpdate` callback flow;
  unit-test through the UniFFI seam against fixture adapters.
- **M2 — Lift streaming orchestration.** Move `LiveStreamState`/`handle_snippet`/
  `finalize_streamed_take`/`choose_final_take_text` from `idiolectd/run_loop.rs`
  **up into `idiolect-application`** (shared = no Android re-implementation =
  guards the streaming-drops-words bug). A desktop-benefiting refactor, landed
  test-first, before any Android code uses it.
- **M3 — Audio + IME bring-up.** `idiolect-adapter-android-audio` (AudioRecord+JNI)
  + `idiolect-adapter-android-ime` (InputConnection callbacks); `MicForegroundService`;
  `IdiolectImeService` with mic key + inline correction strip; privacy gate. E2E:
  tap mic → partials via `setComposingText` → finalise via `commitText`.
- **M4 — Correction capture + storage.** Read the field via `InputConnection`
  ground truth; write pairs (`commit_session`/`amend_correction`); move the crypto
  key to the Keystore.
- **M5 — Model management.** Authenticated base-model download (progress/resume) +
  Zip-Slip + per-file SHA-256 (extract + every load); lazy model init on focus.
- **M6 — Sync round-trip.** `idiolect-sync-client` outbox pump (WorkManager) over
  Tailscale with delete-after-ACK; PC runs `trainerctl` unchanged; `GET /model`
  pulls the personalised tiny/base `.bin`; swap on next focus. Gate auto-ship
  behind the existing (currently-unwired) promotion WER policy.

---

## Risks

- **The shared `--workspace` gate vs Android-only crates** *(highest)* — adding
  the FFI/Android crates to the default host build would break `cargo test
  --workspace`/`cargo clippy --workspace --all-targets` on x86_64. **Mitigation:**
  keep them out of `members`, `cfg`-gate to `target_os = "android"`, keep all FFI
  in safe Rust via UniFFI (respect `unsafe_code = forbid`), add a *separate*
  aarch64 cargo-ndk CI job.
- **whisper.cpp aarch64-NDK build** — the single heaviest dependency (CPU/NEON
  only). Build-plumbing risk, not architecture.
- **Pipeline drift** — if the streaming orchestration is duplicated instead of
  lifted, the streaming-drops-words bug returns and poisons training pairs.
- **`audio_digest` not set in prod today** — must be added at capture (desktop +
  mobile) or dedup/validation breaks.
- **Sync transport must self-encrypt** — at-rest covers only `history.text` and is
  per-device; rely on TLS for the wire.
- **Onboarding friction** — enabling an IME triggers Android's "this keyboard may
  collect all text you type" warning; mitigated by the privacy-first/on-device story.
- **Licensing** — see below.

---

## Open questions (need a decision)

1. **Ingest host:** does `idiolectd` take on an async HTTP stack (axum/hyper)
   beside its blocking Unix-socket loop, or is ingest a **separate
   `idiolect-sync-server`** binary writing into the same data-root? (Leaning
   separate binary to keep the daemon lean.)
2. **`base_model_id` mismatch:** the phone's raw transcripts come from its
   on-device model (tiny/base) which may differ from the PC training base. Train
   against the phone's base, or `revalidate --model <pc-base>` to normalise raw
   transcripts first?
3. **Default on-device model size** — `base.en` Q5_1 (~57 MB) recommended; offer
   `small` opt-in?
4. **Model-back bandwidth:** each merged personalised model is a full base copy
   (W += B·A at f16), not a small delta. Accept, or build "serve adapters without
   merging" before mobile model-return?
5. **Tailscale vs Headscale** (third-party control plane vs self-hosted) and
   whether to also support a **LAN-only mDNS** fallback for home Wi-Fi.
6. **Promotion gate:** `evaluate_promotion` exists but isn't wired into `train`
   (only holdout-loss is enforced). Wire the automated WER gate for the mobile
   round-trip, or keep manual promotion on the PC for now?
7. **Where the Gradle/Kotlin tree lives:** in-tree `android/`, or a sibling that
   consumes the `.so` as an AAR? (Recommend in-tree.)
8. **`v1`/protocol versioning** and replay protection for the endpoint
   (security-review owned).

---

## Licensing note

Idiolect is **AGPL-3.0-only**. whisper.cpp/ggml is **MIT** (fine to bundle).
WhisperVault is **GPL-3.0** — borrow *concepts*, do not copy its source, or the
mobile app inherits GPL. AGPL-3.0 is one-way compatible with GPL-3.0; confirm the
combined-work obligations before publishing, especially if the Android app links
any GPL/Apache Kotlin libraries.

> **Related repo observation (not part of this plan):** the `.deb` is currently
> **Fcitx5-based** (ships `idiolect.so`), while active development is on the
> **IBus** engine (`ibus-engine-idiolect`). Worth reconciling the desktop
> packaging story independently of mobile.

---

## References

- WhisperVault: <https://github.com/nick-tgcs/WhisperVault> (fork of
  `woheller69/whisperIMEplus`).
- whisper.cpp: <https://github.com/ggml-org/whisper.cpp> (models/sizes:
  `models/README.md`); whisper-rs: <https://docs.rs/crate/whisper-rs/latest>.
- Android IME: <https://developer.android.com/develop/ui/views/touch-and-input/creating-input-method>;
  FGS types: <https://developer.android.com/develop/background-work/services/fgs/service-types>.
- cargo-ndk: <https://github.com/bbqsrc/cargo-ndk>; UniFFI:
  <https://github.com/ianthetechie/uniffi-starter>.
- Tailscale: <https://tailscale.com/>; Headscale: <https://github.com/juanfont/headscale>;
  Syncthing folder-types (why not Syncthing): <https://docs.syncthing.net/users/foldertypes.html>.
- Internal seams: `idiolect-ports/src/input_method.rs`,
  `idiolect-trainerctl/src/manifest.rs` (`ManifestV2Item`),
  `idiolect-adapter-whisper/src/lib.rs`, `idiolect-adapter-opus` (IDOPUS1),
  `idiolect-common/src/config.rs` (`XdgBaseDirs`), `idiolectd/src/run_loop.rs`
  (streaming orchestration to lift).
- Related future docs: [003 — Global Hotkey](003-global-hotkey.md),
  [008 — AI Post-processing](008-ai-post-processing.md). Project memory:
  *Burn LoRA training plan*, *streaming drops words*, *UX one surface*.
```
