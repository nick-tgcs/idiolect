# 009 — Android Implementation Plan

**Status:** Active (branch `feat/android-mobile`)
**Companion to:** [009 — Android Mobile (architecture & decisions)](009-android-mobile.md)
**Effort:** Large (multi-phase, TDD throughout)

> This is the *build* doc. [009-android-mobile.md](009-android-mobile.md) decided
> **what** and **why** (monorepo, whisper.cpp/ggml, IME-as-port, Tailscale sync,
> train-on-PC). This doc specifies **how**: the mobile UX in detail, the exact
> code seams to move/extend (with file:line), the phased TDD task list, and an
> **emulator-driven test strategy that exercises every layer** — because the
> user's two explicit asks this round were *"really good UX on mobile"* and
> *"leverage the emulator to test all the functionality."*

---

## Decisions locked this round

From the product Q&A (2026-06-15):

| Decision | Choice | Consequence for the plan |
|---|---|---|
| **Keyboard scope** | **Voice-first, *with* a real text keyboard**, and a **dead-simple toggle** between them. "Leverage existing keyboard functionality to start." **Swipe/glide = future nice-to-have.** | Two in-place modes in one IME view (no window swap). v1 edit-mode QWERTY is tap-only; the 🌐 key also hands off to the user's other keyboard as an instant fallback. Swipe is a roadmap item (borrow FlorisBoard's glide engine, Apache-2.0 — verify license before lifting). |
| **Distribution** | **Personal sideload first.** | Self-signed APK, model pulled from the user's **own PC** over Tailscale, no Play data-safety/App-Signing work now (perfect fit for GrapheneOS — no Play Store dependency). Runtime `RECORD_AUDIO` + IME-enable UX still required. |
| **Min OS** | **Android 12 / API 31.** | One clean foreground-service-type path (`microphone`), modern runtime-permission + mic-dot model, smaller emulator/test matrix. `targetSdk` tracks latest (36). |
| **Target OS** | **Must run on GrapheneOS** (de-Googled, hardened). | No Google Play Services, network-optional, `hardened_malloc`-safe native libs, FOSS-only. Full requirements in [§GrapheneOS](#grapheneos-compatibility--a-hard-target). |

These resolve open questions #3, #5, #6 partially and shape #7 (in-tree `android/`).
Remaining open questions are resolved with recommendations in
[§9](#9-resolved-open-questions).

---

## GrapheneOS compatibility — a hard target

The first-class deployment target is **GrapheneOS** (de-Googled, hardened Android).
This *validates* the locked choices (on-device ASR, Tailscale, sideload, FOSS) and
adds hard requirements + tests. Net win: a fully on-device, no-telemetry,
network-optional voice keyboard is exactly what a GrapheneOS user wants — Gboard
and friends are non-starters there.

**Hard requirements (must hold — not aspirational):**

1. **Zero Google Play Services dependency.** No FCM/GCM push, no Play App Signing,
   no GMS-gated APIs. Background work is **AndroidX `WorkManager`** (no GMS);
   sync is **poll/outbox**, never push. *Assert it:* a CI check that the release
   APK's dependency graph contains no `com.google.android.gms`/`firebase` and the
   manifest declares no GMS components. Must install & run **without** sandboxed Play.
2. **Network-optional, degrades gracefully.** GrapheneOS can revoke the per-app
   **Network** permission. Everything except sync works fully offline; with network
   denied the outbox just queues and the UI says "sync paused — no network
   permission." `INTERNET` is the only network perm; pairing/sync are strictly
   user-initiated.
3. **Native libs clean under `hardened_malloc`.** whisper.cpp/ggml, opus and SQLite
   are C/C++ and run under GrapheneOS's hardened allocator, which *crashes* on heap
   bugs the stock allocator tolerates (OOB, UAF, double-free). We must run clean
   **without** the per-app *Exploit protection compatibility mode*. *Mitigation:*
   build native libs with `-D_FORTIFY_SOURCE=2`, run the Rust+FFI suite under
   **ASan/UBSan** in CI, and gate releases on a **real GrapheneOS device smoke**
   (§6). Needing compat mode is treated as a bug, not a workaround.
4. **W^X / no runtime code generation.** GrapheneOS forbids writable-executable
   memory and executing code from app-writable storage. We are safe: the ASR stack
   is **AOT-compiled** (ggml CPU backend, no JIT), we load model **data**
   (SHA-256-verified) not code, and never `dlopen` a downloaded `.so`. *Assert it:*
   no executable model/plugin-loading path exists — models are data only.
5. **Tailscale via `VPNService`** works on GrapheneOS (no GMS). Document the Android
   single-active-VPN / always-on-VPN caveat. **Elevate the LAN-only mDNS fallback**
   from "later" to near-term so home-Wi-Fi sync needs no VPN at all (helps users who
   keep a different always-on VPN).
6. **Minimal, toggle-friendly permissions.** Only `RECORD_AUDIO`,
   `FOREGROUND_SERVICE` + `FOREGROUND_SERVICE_MICROPHONE`, and `INTERNET`. No
   contacts/location/storage/sensors. App-private `filesDir` only (scoped storage,
   no `MANAGE_EXTERNAL_STORAGE`). Works with GrapheneOS Storage Scopes / Sensors /
   Network toggles without special-casing.
7. **FOSS-only, F-Droid-eligible.** No proprietary SDKs/analytics. Stack stays
   AGPL app + MIT whisper.cpp + BSD opus + public-domain SQLite — keeps the door
   open to reproducible F-Droid builds later (not in scope for sideload-first).

The stock AOSP emulator cannot reproduce `hardened_malloc`, the Network-permission
toggle, the `VPNService` path, or W^X enforcement — so a **GrapheneOS real-device
smoke is a required pre-release gate** (§6.3).

---

## 1. Product UX spec (the hero)

### 1.1 Principles

1. **Voice is the hero.** The default surface is built for one-tap dictation; the
   keyboard is a *correction* tool one tap away, never a tax on the common path.
2. **One surface, two modes, no jump.** Voice mode and edit mode swap *in place*
   at identical height — no new window, no relayout flicker. (Honours the repo's
   [one-surface rule](009-android-mobile.md#android-runtime-architecture).)
3. **Correcting is the product, not an afterthought.** When the model mishears,
   fixing it must be trivial *and* that fix is the training signal we ship to the
   PC. The correction UX and the learning-capture path are the same code path.
4. **The recording indicator never lies.** The mic state shown is the
   **authoritative `RecordingStatus` pushed from Rust** — the UI never optimistically
   flips its own indicator (preserves the desktop single-source-of-truth invariant;
   see memory *streaming drops words* / *UX one surface*).
5. **Privacy is visible.** Mic stops the instant the field is hidden; the on-screen
   state plus Android 12's system mic dot make "are we listening?" unambiguous.

### 1.2 Voice mode (default)

```
┌──────────────────────────────────────────────┐
│  ● listening        ▁▂▅▇▅▂▁  (live waveform)   │ ← authoritative status + RMS meter
├──────────────────────────────────────────────┤
│  "send him the …"            (live preedit)    │ ← setComposingText partials
├──────────────────────────────────────────────┤
│                                                │
│                  ╭────────╮                    │
│                  │   ●    │   MIC              │ ← tap = toggle, hold = push-to-talk
│                  ╰────────╯                    │
│                                                │
├──────────────────────────────────────────────┤
│  ⌨ keyboard   ⌫   space   ⏎   🌐 switch        │ ← compact control row
└──────────────────────────────────────────────┘
```

- **Mic key.** Tap to start, tap to stop (toggle). **Long-press = push-to-talk**
  (hold to talk, release to finalize). Three visual states driven by Rust:
  `idle` → `listening` (animated waveform from capture-thread RMS) → `decoding`
  (spinner while the full-take re-decode runs).
- **Status line + waveform** = the `RecordingStatus` push. Decoding state shown
  explicitly so the user knows the final (authoritative) decode is in flight.
- **Live preedit** appears in the host field via `setComposingText` as snippets
  decode; the **stop-time full-take re-decode is authoritative** and replaces it
  via `commitText` (carries the *streaming-drops-words* fix to Android).
- **Control row.** `⌨` flips to edit mode; `⌫`/`space`/`⏎` cover the 90% of small
  fixes without leaving voice; `🌐` is the system IME switcher (instant handoff to
  the user's full keyboard — this *is* "leverage existing keyboard to start").

### 1.3 Edit mode (one tap away)

```
┌──────────────────────────────────────────────┐
│  raw:  "their"   →   tap a word to fix         │ ← correction strip (post-take)
├──────────────────────────────────────────────┤
│  q  w  e  r  t  y  u  i  o  p                   │
│   a  s  d  f  g  h  j  k  l                     │
│  ⇧   z  x  c  v  b  n  m   ⌫                    │
│  123   🎤   space   .   ⏎                       │ ← 🎤 jumps back to voice mode
└──────────────────────────────────────────────┘
```

- **v1 = tap-only QWERTY.** No autocorrect/prediction engine (deliberately — keep
  the surface small and predictable). Standard editing through `InputConnection`.
- **`🎤` returns to voice mode** in place. The toggle is symmetric and always one
  tap, from either mode — the user's stated hard requirement.
- **Swipe/glide = future.** Tracked in [§8](#8-risks--spikes). Until then, `🌐`
  hands off to the user's preferred keyboard for heavy typing.

### 1.4 Correction flow = training capture (the crux)

This is where "fix it simply" and "ship a learning to the PC" become one path:

1. A take commits to the host field; the **correction strip** renders the committed
   words as tappable chips.
2. **Tap a wrong word** → the IME selects that word's range in the field
   (`InputConnection.setSelection`) and flips to **edit mode** with the word
   selected, so the next keystroke replaces it. (Alt affordance: *re-speak this
   word* → a short scoped dictation.)
3. On commit of the fix, Rust records the **raw→corrected pair** via the existing
   [`amend_correction`](crates/idiolect-adapter-sqlite/src/repository.rs#L996)
   path — identical contract to desktop. That row + its audio is exactly one
   `SyncLearning` ([§4](#4-architecture-deltas-grounded-in-code)).
4. Ground truth for the corrected text is read back from the field via
   `InputConnection.getTextBeforeCursor/Selected` (mirrors the desktop "read the
   field" capture), so we never trust our own optimistic state.

> ⚠️ History edits must also update the `ime_text_history` projection, not just
> the session text (memory *history projection gotcha*). The Android history
> screen reuses the same mutators, so it inherits the fix — and must be tested for it.

### 1.5 Onboarding (first run, companion Activity)

Permissions and dialogs **cannot** be shown from the IME — they run in the
companion Activity:

1. **Welcome / privacy** — on-device, personal, your-own-PC framing.
2. **Enable the keyboard** — deep-link to system IME settings; pre-empt Android's
   *"this keyboard can collect all the text you type"* scare with our own honest
   explanation first.
3. **Select Idiolect** as the active input method (guide to the IME switcher).
4. **Grant microphone** — runtime `RECORD_AUDIO`, requested from the Activity.
5. **Download the base model** — `base.en` Q5_1 (~57 MB), progress + resume +
   SHA-256 verify. Source = the user's **own PC** sync-server (personal build).
6. **Pair with the PC** — scan the QR the desktop settings shows; mint a per-device
   bearer token into the Android Keystore.
7. **Try it** — a sample field to dictate into.

### 1.6 Companion Activity (settings / history / sync)

A normal Compose Activity, separate from the IME (the input view is too small and
can't host dialogs):

- **Model management** — download/switch `tiny`/`base`/`small`; integrity status;
  active model + version.
- **Sync & pairing** — pairing state, last sync, **pending learnings count**,
  **storage reclaimed**, manual "sync now".
- **History/review** — list past takes; **edit a past entry** (mirrors the desktop
  review dialog; uses the same mutators incl. the `ime_text_history` projection).
- **Privacy** — delete-all, mic behaviour, what leaves the device.

### 1.7 Cross-cutting UX requirements

- **Accessibility:** TalkBack labels on every key/state; mic key ≥ 56 dp; the
  recording state is announced and high-contrast (not colour-only).
- **Theming:** Material 3, follow system dark/light + dynamic colour.
- **Orientation/one-handed:** mic reachable; compact landscape input view.
- **Audio interruptions:** request `AudioFocus`; pause/stop capture on incoming
  call or focus loss; route to BT/wired headset mic when present.
- **Haptics:** light tick on mic start/stop and on commit.
- **Privacy gate (hard invariant):** stop capture on `onFinishInputView` /
  `onWindowHidden` / `inputType == TYPE_NULL`. Test this explicitly.

---

## 2. What's already portable (good news from the code map)

The exploration confirmed the hexagon holds and some lifting is *already done*:

- **Segmentation primitives are already in `idiolect-application`** —
  `FrameBuffer`, `SegmenterConfig`, `UtteranceSegmenter`, `Snippet` live in
  [segmentation.rs](crates/idiolect-application/src/use_cases/segmentation.rs#L18-L211)
  and are pure/no-I/O. Android reuses them directly; only the *orchestration around*
  them is still daemon-bound.
- **GPU is a clean cargo feature** —
  [`GPU_ENABLED = cfg!(feature = "cuda")`](crates/idiolect-adapter-whisper/src/lib.rs#L15);
  an Android build that omits `cuda` runs CPU with **zero logic change**. Decode is
  already full-segment beam search with
  [`no_timestamps`](crates/idiolect-adapter-whisper/src/lib.rs#L179) — exactly the
  VAD-then-full-decode shape mobile needs.
- **SQLite is bundled** ([Cargo.toml](Cargo.toml#L66), `features=["bundled"]`) — no
  system sqlite on device.
- **Audio store is already behind a port** —
  [`AudioStorePort`](crates/idiolect-ports/src/storage.rs#L26-L53); the path
  abstraction is in place.
- **Config already supports path injection** —
  [`StorageConfig.data_dir` / `database_path`](crates/idiolect-common/src/config.rs#L264-L267)
  let Kotlin pass `filesDir`-based paths without touching XDG logic at all (the
  cheap seam; the `PathProvider` trait is the clean version).

---

## 3. What must change on the **desktop** first (and benefits desktop)

These land before any Kotlin exists, are pure-Rust TDD, and fix/strengthen the
desktop product:

1. **Populate `utterances.audio_sha256` in production capture.** Today it is
   **test-only** — written solely by
   [`set_audio_digest_for_test`](crates/idiolect-adapter-sqlite/src/repository.rs#L1305-L1318);
   the trainer reads it as
   [`COALESCE(u.audio_sha256, '')`](crates/idiolect-adapter-sqlite/src/repository.rs#L1214)
   and [`build_v2` rejects an empty digest](crates/idiolect-trainerctl/src/manifest.rs#L345).
   Compute SHA-256 of the IDOPUS1 payload at
   [`write_source_audio`](crates/idiolect-adapter-sqlite/src/audio_store.rs#L461-L478)
   / [`commit_session`](crates/idiolect-adapter-sqlite/src/repository.rs#L2022)
   time. **This also unblocks real manifest validation on desktop today.**
2. **Add a `synced` status — no migration needed.** `training_candidates.status`
   is freeform `TEXT DEFAULT 'captured'`
   ([migration](crates/idiolect-adapter-sqlite/migrations/0003_v1_storage.sql#L242),
   no CHECK constraint). Add a `TRAINING_STATUS_SYNCED` const beside
   [`TRAINING_STATUS_CAPTURED`](crates/idiolect-adapter-sqlite/src/repository.rs#L32)
   and the update method; the trainer query already filters `status != 'rejected'`
   ([here](crates/idiolect-adapter-sqlite/src/repository.rs#L1223)).
3. **Lift the streaming orchestration out of the daemon** into
   `idiolect-application` so Android reuses it verbatim. The functions/state to
   move (all in [run_loop.rs](crates/idiolectd/src/run_loop.rs)):

   | Item | Location | Note on the lift |
   |---|---|---|
   | `LiveStreamState` (+ `ingest`, `auto_stop_due`, `flush`) | [L1004-L1106](crates/idiolectd/src/run_loop.rs#L1004) | The core streaming state machine. `ingest` is the resample→frame→VAD→segmenter pump. |
   | `handle_snippet` | [L1409-L1490](crates/idiolectd/src/run_loop.rs#L1409) | Per-snippet decode + preedit. Decouple from `UnixStream`: return events, don't write IPC. |
   | `finalize_streamed_take` | [L1499-L1582](crates/idiolectd/src/run_loop.rs#L1499) | **The authoritative full-take re-decode.** Split persistence (`store`/`codec`) out so the caller persists. |
   | `choose_final_take_text` / `snippet_chunk` / `is_noise_transcript` | [L2206](crates/idiolectd/src/run_loop.rs#L2206) / [L2190](crates/idiolectd/src/run_loop.rs#L2190) / [L2153](crates/idiolectd/src/run_loop.rs#L2153) | Pure helpers — move as-is. |
   | `StreamingResampler` | [adapters.rs L253-L300](crates/idiolectd/src/adapters.rs#L253) | Pure DSP — move to `idiolect-application` (or `ml-core`) and make public. |

   The orchestration becomes I/O-free: it takes an `&dyn AsrPort` and emits a
   stream of *events* (`Preedit`, `Commit`, `RecordingStatus`, `Error`) instead of
   writing to a `UnixStream`. Desktop's `run_loop` then becomes a thin adapter
   that maps those events to IPC; Android maps them to UniFFI callbacks. **This is
   the single most important anti-drift move** — it makes the *streaming-drops-words*
   bug structurally impossible to reintroduce on either platform.

   Ports unchanged and reused: [`AudioInputPort`](crates/idiolect-ports/src/audio.rs#L38-L50)
   (`start/stop/poll_captured` — `poll_captured` is the streaming seam),
   [`AsrPort`](crates/idiolect-ports/src/asr.rs#L14-L19),
   [`InputMethodPort`](crates/idiolect-ports/src/input_method.rs#L3-L10).

---

## 4. Architecture deltas grounded in code

New/changed crates (refines [009 §Concrete layout](009-android-mobile.md#concrete-layout)):

```
crates/idiolect-application/src/use_cases/streaming.rs   # NEW: lifted orchestration (event-emitting, I/O-free)
crates/idiolect-sync/            # ✅ DONE: shared wire types SyncLearning/SyncBatch + length-prefixed binary container codec
crates/idiolect-sync-client/     # ✅ DONE (logic): build_batch (outbox→envelope) + confirm_shipped (ACK→reclaim)
crates/idiolect-sync-server/     # ✅ DONE (ingest logic): envelope→rows+audio, idempotent. HTTP/GET-model still TODO (axum)
crates/idiolect-mobile-runtime/  # NEW Android twin of run_loop (in-process; maps events→callbacks)
crates/idiolect-ffi/             # NEW the ONE UniFFI facade; only cdylib/.so; kept OUT of workspace `members`
android/                         # NEW Gradle project (sibling, NOT a crate)
```

- **`SyncLearning` DTO** mirrors
  [`ManifestV2TrainingCandidate`](crates/idiolect-adapter-sqlite/src/repository.rs#L200-L211)
  field-for-field but adds `Serialize + Deserialize` and **omits `split`** (the
  PC owns train/val/holdout). `ManifestV2Item` itself stays Serialize-only/private
  ([manifest.rs L136](crates/idiolect-trainerctl/src/manifest.rs#L136)); the wire
  type is deliberately separate.
- **`audio_digest`** = SHA-256 of the IDOPUS1 payload
  ([format](crates/idiolect-adapter-opus/src/lib.rs#L82)) — content-addressed dedup
  key *and* ACK token.
- **Delete-after-ship** uses the narrow
  [`delete_source_audio_for`](crates/idiolect-adapter-sqlite/src/audio_store.rs#L180-L188),
  **never** the cascading
  [`prune_training_data`](crates/idiolect-adapter-sqlite/src/repository.rs#L733-L808).
  Flip to `synced`, drop only the Opus, keep row + transcript.
- **Crypto key → Android Keystore.** Desktop
  [`FileKey`](crates/idiolect-adapters/crypto/src/lib.rs#L131-L159) writes a `0o600`
  32-byte file; on Android implement `EncryptionKeyPort` over the Keystore (unix
  perms are meaningless in the sandbox). The
  [`ChaCha20Poly1305Cipher`](crates/idiolect-adapters/crypto/src/lib.rs#L54)
  itself is reused unchanged.
- **PathProvider.** Add a `Platform::Android` arm to
  [`platform_defaults`](crates/idiolect-common/src/config.rs#L482-L500) and
  [`Platform::host`](crates/idiolect-common/src/config.rs#L450), or (cheap interim)
  just inject `data_dir`/`database_path` from Kotlin.

---

## 5. Phased plan (TDD; red → green → refactor at every step)

Ordered so each phase is independently green and the **desktop benefits first**.
Every behaviour gets unit + integration + e2e coverage unless a level is genuinely
unreachable (the Compose `onCreateInputView` render is the one declared GUI seam,
mirroring the IBus/eframe caveats). Gates stay green throughout:
`cargo test --workspace` + `cargo clippy --workspace --all-targets` + `cargo fmt --all --check`.

### Sync-protocol track (desktop-side, no Android required)

- **S0 — Digest + status foundations.** ✅ **S0a + S0b done.** Compute & store
  `audio_sha256` in production capture (`idiolect_common::digest::audio_sha256_hex`
  → `persist_session` → `SqliteMetadataStore::set_audio_digest`); new `idiolect-sync`
  crate with `SyncLearning`/`SyncBatch` + length-prefixed binary container codec.
  *Tests (all green):* unit round-trip of the codec & DTOs (+ framing-error cases);
  integration proving capture now populates the digest (`run_loop::tests::capture_persist`);
  e2e proving a production-computed digest flows through `build_v2`. **Still TODO:**
  the `synced` status const + setter (rolls into S1). **Exit (met):** desktop
  manifest validation works on real captures.
- **S1 — Delete-after-ship locally.** ✅ **Done.** `TRAINING_STATUS_SYNCED` +
  `mark_synced_and_drop_audio` (status flip committed *before* the narrow
  `delete_source_audio_for`) + the outbox query `training_candidates_pending_sync`
  (`status = 'captured'`); the manifest feed now excludes `synced` too (shared
  `collect_candidates` helper). *Tests (green, [sync_reclaim.rs](crates/idiolect-integration-tests/tests/sync_reclaim.rs)):*
  integration — audio file gone, row+transcript survive, synced candidate leaves
  both manifest and outbox while the un-synced one is untouched and still trains;
  unknown candidate errors. **Exit (met):** storage reclaim proven on desktop.
- **S2 — Transport + ingest on one box.** ✅ **Logic done; HTTP/CLI remaining.**
  `idiolect-sync-client` (`build_batch`/`confirm_shipped`) + `idiolect-sync-server`
  (`ingest`, content-addressed idempotent). The whole protocol is proven on one
  box *in-process* by [sync_round_trip.rs](crates/idiolect-integration-tests/tests/sync_round_trip.rs):
  capture+correct in data-root A → build → encode→decode (wire codec) → ingest into
  data-root B → corrections land as trainable candidates with audio intact →
  reclaim on A; replay is idempotent. **Remaining:** the actual HTTP transport
  (axum `POST` + `GET /model`) and an `idiolect-cli sync push` — and (nice-to-have)
  extending the e2e through `trainerctl train` on B to a merged `.bin`. **Exit
  (partially met):** protocol logic validated before any Kotlin; only the network
  hop is left.
- **S3 — Auth + pairing + idempotency.** QR/code handshake → per-device bearer
  token; `(device_id, audio_digest)` dedup; at-rest outbox encryption.
  *Tests:* unit (token bind/verify), integration (replayed batch is idempotent,
  bad token rejected). **Exit:** safe to expose on the tailnet.

### Mobile track

- **M0 — Build plumbing (the real cost).** ✅ **Cross-compile proven.** With
  **NDK r28** + cargo-ndk v4, the whole portable core builds for **both**
  `aarch64-linux-android` (device) and `x86_64-linux-android` (emulator) via
  [scripts/android-cross-build.sh](scripts/android-cross-build.sh) — whisper.cpp
  (CMake), opus/`audiopus_sys` (CMake), bundled SQLite, and webrtc-vad all
  cross-compile clean. **Both spikes retired** ([§8](#8-risks--spikes)):
  `opus-sys` needed only the right NDK env (`ANDROID_NDK_ROOT`/`ANDROID_NDK`, not
  just `_HOME`); whisper.cpp built in ~20 s with host libclang for bindgen. A dead
  `usearch`/`numkong` C++ dep (via unused `sqlite-vector-rs` in the sqlite
  adapter) was **removed** — it broke the Android C++ build and bloated desktop.
  ✅ **Run half also proven.** The full portable core *executes* on the x86_64
  API-33 emulator via [scripts/android-emulator-test.sh](scripts/android-emulator-test.sh):
  **25 test-groups, 0 failures** — bundled SQLite (incl. encrypted history,
  event-sourcing, migrations, the audio-digest + sync work), webrtc-vad, opus, the
  application/common logic, the sync codec, **and 8 whisper tests including the
  real fixture *decode* (whisper.cpp transcribing on-device, ~21 s; same assertion
  passes host + device = decode parity).** `libc++_shared.so` is pushed to the
  device and rpath'd in (whisper links it dynamically). **M0 exit met at the proof
  level.** Housekeeping still owed (rolls into M1/M3): the actual cdylib/`.so`,
  bundling `libc++_shared` in the APK, an LTO-off release profile, and the CI
  jobs (compile-only + emulator).
- ✅ **M1 — PathProvider + UniFFI facade — done.** `idiolect-ffi` (UniFFI 0.31,
  proc-macro mode) exposes `IdiolectCore` (`toggle/commit/cancel/report_correction/
  push_pcm_frame` + `recent_history/history_edited/reinsert_history/
  open_history_edit/is_recording`) and the `IdiolectInputMethod` callback
  (`recording_status/show_preedit/update_preedit/commit_text/cancel_preedit/
  insert_text/edit_history`). It drives the **unchanged** `DictationUseCase` over a
  real `SqliteMetadataStore` — the in-process collapse of the daemon's socket IPC.
  `PathProvider` (`config.rs`): `XdgPaths` (desktop) + `RootedPaths` (Android
  `filesDir`). The cdylib cross-compiles to **arm64-v8a + x86_64** with bundled
  `libc++_shared.so` and generated Kotlin bindings
  ([android-ffi-build.sh](scripts/android-ffi-build.sh)) — the `.so` M0 deferred.
  *Tests:* 8 host **seam** tests through the exported surface + the callback trait
  against a real SQLite store, plus `PathProvider`/`storage_mut` unit+contract
  tests. Streaming decode (PCM→text) is deliberately **out** (M2): `push_pcm_frame`
  buffers and `IdiolectCore::deliver_transcript` is the M2 hook (test-driven now).
  **Two deliberate divergences from [009](009-android-mobile.md):** (a) `idiolect-ffi`
  is **in** the workspace `members` (it is host-buildable pure-Rust+UniFFI, so the
  mandatory `cargo test/clippy --workspace` gates cover the seam); only the
  genuinely Android-only adapters (M3) stay out. (b) UniFFI's generated scaffolding
  emits `unsafe`, which the workspace `forbid(unsafe_code)` cannot allow-away, so
  the crate sets its **own** `[lints]` (deny-warnings, no `forbid`) — all
  hand-written code there is still safe.
- **M2 — Lift streaming orchestration.** Execute [§3.3](#3-what-must-change-on-the-desktop-first-and-benefits-desktop)
  into `idiolect-application/streaming.rs`; rewire desktop `run_loop` onto it
  (proves the refactor is behaviour-neutral). *Tests:* the existing daemon
  streaming tests must pass unchanged against the lifted module; new unit tests
  on the event stream (esp. the full-take-wins logic). **Exit:** desktop green on
  the shared orchestration; *streaming-drops-words* covered by a regression test.
- **M3 — Audio + IME bring-up.** `idiolect-adapter-android-audio` (AudioRecord +
  JNI PCM push) + `idiolect-adapter-android-ime` (InputConnection callbacks);
  `MicForegroundService` (`foregroundServiceType=microphone`); `IdiolectImeService`
  with the **voice mode** view + privacy gate. *Tests:* Robolectric on the service
  logic; Compose UI test on the input view states; **emulator e2e** (fixture
  audio → `commitText`, see [§6](#6-emulator--testing-strategy)).
- **M4 — Edit mode + correction capture.** The QWERTY edit mode, the **one-tap
  toggle**, the correction strip + tap-to-fix selecting the word range; wire fixes
  to `amend_correction` (incl. the `ime_text_history` projection). Crypto key →
  Keystore. *Tests:* Compose UI test for the toggle + tap-to-fix; integration that
  a fix produces a correct raw→corrected candidate; the history-projection
  regression test.
- **M5 — Model management.** Authenticated model download (progress/resume) from
  the user's PC; Zip-Slip hardening; per-file SHA-256 at extract **and** every
  load; lazy model init on first focus; model switch (tiny/base/small). *Tests:*
  Rust unit on verify/extract (tamper → reject); instrumented download/resume.
- **M6 — Sync round-trip.** `idiolect-sync-client` outbox pump via `WorkManager`
  over Tailscale with delete-after-ACK; PC runs `trainerctl` unchanged;
  `GET /model` pulls the personalised `.bin`; atomic swap on next focus. *Tests:*
  full e2e — dictate+correct on the **emulator** → sync to a local PC server →
  train → pull → swap → next decode reflects it; assert phone storage reclaimed.
- **M7 — UX polish & a11y.** Waveform/haptics/decoding state; TalkBack pass;
  dark/dynamic theming; landscape; audio-focus interruptions; onboarding flow.
  *Tests:* screenshot tests per state ([§6](#6-emulator--testing-strategy)); a
  TalkBack/accessibility instrumented sweep.

---

## 6. Emulator & testing strategy

> Goal: **exercise every layer, most of it deterministically and headless.** An
> IME + mic + native core is awkward to test, so the strategy pushes logic down to
> host-runnable Rust/JVM tests and reserves the emulator for the genuinely
> device-bound seams — with a **fixture audio source** so even the "voice" e2e is
> deterministic.

### 6.1 Test pyramid

| Level | Where it runs | What it covers | Emulator? |
|---|---|---|---|
| **Rust unit** | host x86_64 | brain, lifted streaming orchestration, sync DTOs/codec, outbox/ACK-delete, digest, model verify | no |
| **Rust integration** | host x86_64 | sync round-trip on one box (S2), train→pull, capture→digest→manifest | no |
| **Rust-on-Android parity** | emulator (x86_64 `.so`) | the cross-built core decodes the desktop fixture to **identical tokens** | yes (instrumented) |
| **Kotlin unit (Robolectric)** | JVM | IME service logic, mode toggle, status mapping, view-models | no |
| **Compose UI tests** | emulator | input-view states, toggle, correction strip, onboarding/settings Activity | yes |
| **IME e2e (fixture audio)** | emulator | enable IME → focus a field → mic → assert text committed via InputConnection | yes |
| **Screenshot/snapshot** | emulator/JVM | every input-view state × light/dark locked | yes (or Robolectric/Roborazzi) |
| **Sync e2e** | emulator + local PC server | dictate+correct → ship → train → pull → reflected; storage reclaimed | yes |
| **Macrobenchmark** | emulator + 1 real device | decode RTF, mic-start latency vs targets | partial |

The **bulk of correctness lives in host Rust tests** (the whole brain + sync +
streaming). The emulator validates the *edges* the host can't: cross-compiled
decode parity, the IME/InputConnection wiring, and the Compose UX.

### 6.2 Deterministic "voice" e2e via a fixture capture seam

Real mic injection into the emulator is flaky for CI. Mirror the desktop's
existing fixture adapters (`FixtureStream`,
[adapters.rs](crates/idiolectd/src/adapters.rs#L125)): ship a **debug build flavor**
whose `AudioInputPort` is a **fixture that replays a bundled WAV** instead of
`AudioRecord`. The e2e then:

1. `adb shell ime enable <id>` + `adb shell ime set <id>` (and
   `settings put secure default_input_method <id>`) to make Idiolect active.
2. UiAutomator opens a harness Activity with an `EditText`, focuses it (IME shows).
3. Tap the mic key → the fixture replays a known WAV → assert the **known
   transcript** is committed to the field (read back via the test harness).
4. Tap a word → edit mode → replace → assert the raw→corrected candidate is stored.

This makes the "does dictation end-to-end work?" test **fully deterministic** and
CI-safe. **Real-mic injection** (emulator "Virtual microphone uses host audio
input", or piping a WAV via the emulator audio path) is kept as a **separate,
gated manual smoke** — valuable but not a CI gate.

### 6.3 Emulator/AVD matrix & automation

- **AVDs:** API 31 (min OS) + latest (API 36) system images. Use the **x86_64**
  image on Linux CI (matching the `x86_64-linux-android` `.so`); ARM64 image on
  Apple-silicon if available.
- **Local automation:** `scripts/android-emulator-e2e.sh` — boots a headless AVD
  (`-no-window -no-audio` for the fixture path), installs the debug APK, enables +
  sets the IME, runs the UiAutomator + Compose suites, tears down. **Runnable by CI
  and by me without any manual clicks.**
- **CI:** GitHub Actions `reactivecircus/android-emulator-runner` for the
  instrumented jobs; a separate `cargo-ndk` **compile-only** `aarch64` job guards
  the device ABI; plus a **no-GMS assertion** job (fails if the APK graph contains
  `com.google.android.gms`/`firebase`) and an **ASan/UBSan** run of the Rust+FFI
  native code (hardened_malloc proxy). All are **new jobs**, kept off the existing
  host `--workspace` gate so x86_64 CI stays green (Android crates stay out of
  workspace `members`, per [009](009-android-mobile.md#concrete-layout)).
- **GrapheneOS device smoke (required pre-release gate).** The stock emulator can't
  reproduce `hardened_malloc`, the Network-permission toggle, `VPNService`, or W^X.
  A scripted real-device pass on a Pixel running GrapheneOS covers: install without
  Play; dictate end-to-end (exercises hardened_malloc on whisper.cpp/opus/sqlite);
  revoke Network → graceful offline + queued outbox; pair + sync over Tailscale;
  model download + SHA-256 verify; **and confirm no per-app compat mode is needed.**

### 6.4 Mapping to the repo's 3-level TDD rule

- **Unit:** Rust unit (brain/streaming/sync) + Kotlin Robolectric (IME logic).
- **Integration:** Rust integration (sync on one box) + Rust-on-Android parity +
  Compose UI tests.
- **E2E:** emulator IME-into-EditText with fixture audio + the full sync round-trip.
- **Declared GUI seam:** the actual Compose `onCreateInputView` *rendering* (state
  logic is covered by Compose UI tests; the reason is stated in the test file,
  mirroring the IBus/eframe boundaries).

---

## 7. First concrete steps on `feat/android-mobile`

Pure-Rust, TDD, desktop-benefiting — startable immediately, no Android toolchain:

1. ✅ **S0a — done.** `persist_session` now hashes `encoded.payload` via
   `idiolect_common::digest::audio_sha256_hex` and stores it with
   `SqliteMetadataStore::set_audio_digest`. Covered by `run_loop::tests::capture_persist`
   (integration), `repository_contract` (component), `digest` unit tests, and the
   migrated `manifest_builder_storage` e2e.
2. ✅ **S0b — done.** `idiolect-sync` crate: `SyncLearning`/`SyncBatch`/`SyncBatchEnvelope`
   DTOs + a length-prefixed binary container codec (`encode_batch`/`decode_batch`),
   round-trip and framing-error tests green.
3. ✅ **S1 — done.** `TRAINING_STATUS_SYNCED` + `mark_synced_and_drop_audio`
   (delete-after-ship via the narrow `delete_source_audio_for`) + the
   `training_candidates_pending_sync` outbox; manifest feed excludes `synced`.
4. ✅ **S2 client half — done.** `idiolect-sync-client`: `build_batch`
   (outbox → content-addressed `SyncBatchEnvelope`, round-trips the codec) +
   `confirm_shipped` (ACK → `mark_synced_and_drop_audio` reclaim). Covered by
   [sync_client.rs](crates/idiolect-integration-tests/tests/sync_client.rs).
5. ✅ **S2 server ingest + one-box round-trip — done in-process.**
   `idiolect-sync-server::ingest` + `sync_round_trip.rs` prove the whole protocol
   (build → wire codec → ingest → trainable on B → reclaim on A; idempotent
   replay) without any network library.
6. ✅ **M0 — done (proof level).** Whole native core cross-compiles to arm64-v8a +
   x86_64 ([android-cross-build.sh](scripts/android-cross-build.sh)) **and** runs
   on the emulator incl. real whisper decode ([android-emulator-test.sh](scripts/android-emulator-test.sh),
   25 groups green). Spikes retired; dead `usearch` removed. The existential
   "can it run on the phone?" risk is **answered: yes.**
7. ✅ **M1 — the UniFFI facade — done.** `idiolect-ffi` (UniFFI 0.31) exposes
   `IdiolectCore` + the `IdiolectInputMethod` callback, driving the unchanged
   `DictationUseCase` over a real `SqliteMetadataStore` — the in-process collapse
   of the daemon socket. 8 host seam tests green; the cdylib cross-compiles to
   arm64-v8a + x86_64 with bundled `libc++_shared` and generated Kotlin bindings
   ([android-ffi-build.sh](scripts/android-ffi-build.sh)). `PathProvider`
   (`XdgPaths`/`RootedPaths`) added. See the M1 bullet in §5 for the two deliberate
   divergences (crate **in** `members`; own `[lints]` for UniFFI's generated unsafe).
8. **NEXT (dependency order, mobile track): M2 — lift the streaming orchestration**
   into `idiolect-application/streaming.rs` and rewire the desktop `run_loop` onto
   it (proves behaviour-neutral); the lifted worker becomes the real caller of
   `IdiolectCore::deliver_transcript`, carrying the full-take-wins policy to the
   phone. The S2 HTTP hop (axum) remains a parallel no-Kotlin track. M3 (the actual
   Kotlin IME — the second existential risk) follows M2.

---

## 8. Risks & spikes

- **`opus-sys` on NDK** ✅ *resolved* — `opus = "=0.3.1"` pulls `audiopus_sys`,
  which **vendors** libopus and builds it via CMake. It cross-compiles fine under
  cargo-ndk once the NDK env is complete (`ANDROID_NDK_ROOT`/`ANDROID_NDK`, not
  just `ANDROID_NDK_HOME`). No system libopus, no shim needed.
- **whisper.cpp aarch64-NDK build** ✅ *resolved* — `whisper-rs-sys` builds
  whisper.cpp via CMake under the NDK in ~20 s (CPU/NEON, `cuda`/`vulkan` OFF);
  bindgen uses host libclang. Built clean for both arm64 and x86_64.
- **Dead `usearch`/`numkong` C++ dep** ✅ *resolved* — `sqlite-vector-rs` (an
  **unused** dependency of the sqlite adapter) pulled `usearch`, whose `numkong`
  header redeclares `syscall(...) noexcept` and clashes with Android's
  `unistd.h`. Removed the dep entirely (no references anywhere); unblocks Android
  and slims the desktop build. *Lesson:* audit transitive C++ deps before the
  Android lane — a future `cargo machete`/unused-deps CI check would catch these.
- **Deterministic mic in CI** — mitigated by the fixture capture seam (§6.2);
  real-mic injection stays a gated manual smoke.
- **Pipeline drift** — mitigated structurally by the M2 lift (shared orchestration).
- **Swipe/glide later** — non-trivial; plan to borrow FlorisBoard's glide engine
  (Apache-2.0 — **verify license** before lifting any code). Until then `🌐`
  hands off to the user's keyboard.
- **IME privacy warning** — Android's "can collect all text" scare; mitigated by
  the onboarding pre-explanation and the on-device/personal story.
- **`hardened_malloc` native robustness** *(GrapheneOS, spike alongside M0)* — the
  C/C++ deps must survive the hardened allocator without compat mode. Catch early
  via ASan/UBSan in CI and the GrapheneOS device smoke; if a dep trips it, fix or
  swap the dep rather than relying on the per-app exploit-protection toggle.
- **No-GMS drift** — an AndroidX/3rd-party lib can silently pull
  `play-services-*`/`firebase` transitively, breaking GrapheneOS-without-Play. The
  CI no-GMS assertion (§6.3) is the guard.
- **Tailscale = single active VPN** on Android/GrapheneOS — only one always-on VPN
  at a time. Mitigated by elevating the LAN-only mDNS sync fallback so VPN isn't
  mandatory for home-Wi-Fi sync.

## 9. Resolved open questions

Carrying [009 §Open questions](009-android-mobile.md#open-questions-need-a-decision)
to decisions for this build:

1. **Ingest host →** **separate `idiolect-sync-server` binary** (keeps the daemon's
   blocking Unix loop lean; axum/hyper isolated).
2. **`base_model_id` mismatch →** PC **`revalidate --model <pc-base>`** normalises
   the phone's raw transcripts before training, so the phone's tiny/base raw never
   poisons the corpus.
3. **Default model →** `base.en` Q5_1 (~57 MB); `small.en` opt-in.
4. **Model-back bandwidth →** accept the full merged `.bin` for v1; "serve adapters
   without merging" is a later optimisation.
5. **Transport →** **Tailscale** for v1 (personal/sideload, least friction);
   LAN-only mDNS fallback later.
6. **Promotion gate →** **manual on the PC** for v1; wire the automated WER gate
   (`evaluate_promotion`) for the round-trip later.
7. **Gradle tree →** **in-tree `android/`**, consuming `idiolect-ffi` as an AAR.
8. **Versioning/replay →** `/v1/` + bearer token + `(device_id, digest)` / `batch_id`
   idempotency; full replay-protection review in S3.

## 10. Doc-hygiene note (not blocking)

`docs/future/` has a **numbering collision**: both this Android work and
[009-macos-port.md](009-macos-port.md) use `009`, while
[010-windows-tsf-port.md](010-windows-tsf-port.md) is `010`. Worth renumbering the
platform-port docs into a clean sequence at some point — out of scope here.

---

## References

- Architecture & decisions: [009-android-mobile.md](009-android-mobile.md).
- Streaming to lift: [run_loop.rs](crates/idiolectd/src/run_loop.rs),
  [adapters.rs](crates/idiolectd/src/adapters.rs); already-shared
  [segmentation.rs](crates/idiolect-application/src/use_cases/segmentation.rs).
- Ports: [audio.rs](crates/idiolect-ports/src/audio.rs),
  [asr.rs](crates/idiolect-ports/src/asr.rs),
  [input_method.rs](crates/idiolect-ports/src/input_method.rs),
  [storage.rs](crates/idiolect-ports/src/storage.rs).
- Storage/contract: [repository.rs](crates/idiolect-adapter-sqlite/src/repository.rs),
  [audio_store.rs](crates/idiolect-adapter-sqlite/src/audio_store.rs),
  [manifest.rs](crates/idiolect-trainerctl/src/manifest.rs).
- Portable adapters: [adapter-whisper](crates/idiolect-adapter-whisper/src/lib.rs),
  [adapter-opus](crates/idiolect-adapter-opus/src/lib.rs),
  [crypto](crates/idiolect-adapters/crypto/src/lib.rs),
  [config.rs](crates/idiolect-common/src/config.rs).
- Android: IME service & InputConnection, FGS types, `RECORD_AUDIO`; cargo-ndk;
  UniFFI; `reactivecircus/android-emulator-runner`; FlorisBoard (Apache-2.0,
  glide engine — future). Project memory: *streaming drops words*, *UX one
  surface*, *history projection gotcha*, *quality gates include fmt*.
