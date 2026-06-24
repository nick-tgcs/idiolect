# 009 — macOS Port (full native)

**Status:** Future
**Priority:** High
**Effort:** Large

## Problem

Idiolect is Linux-only. Every platform seam — the input method, the tray, app
identity, service lifecycle, GPU inference, and the base directories — assumes
X11/GNOME + IBus/fcitx5 + systemd + CUDA. There are **zero `cfg(target_os)`
guards** in the workspace today: the core just happens to compile because the
host is always Linux.

The goal is a **full native** macOS port — not an MVP. Specifically: the live
in-app preedit (underlined in-progress text) must work, via a real macOS Input
Method Kit (IMK) component, not a clipboard/CGEvent shim.

## What already ports for free

The architecture is hexagonal (`idiolect-core` / `idiolect-application` /
`idiolect-ports` + adapters), and the IPC contract is transport-agnostic
newline-delimited JSON over a Unix socket. These stay **untouched**:

| Concern | Today (Linux) | macOS |
|---|---|---|
| Core / application / ports | platform-agnostic | no change |
| IPC transport | `UnixListener`/`UnixStream` (`std::os::unix`) | works on macOS as-is |
| Audio | `cpal` | CoreAudio (handle TCC mic permission) |
| ASR | `whisper-rs` + `cuda` feature | add `metal` (+ optional `coreml`) feature |
| GUI helpers | `eframe` (glow + **x11**) | Cocoa (cfg the eframe features per-target) |
| Clipboard | `arboard` | NSPasteboard |
| Storage | `rusqlite` (bundled) | works |

## What needs a macOS sibling

| Concern | Today (Linux) | macOS sibling |
|---|---|---|
| **Input method** | IBus engine (Rust/zbus) **+** fcitx5 addon (C++) | **native IMK component** (see below) |
| **Tray / menu** | `ksni`, in-daemon (DBus/StatusNotifierItem) | **menu-bar agent** (`NSStatusItem`), separate process |
| Caret indicator | egui binary; X11 caret coords supplied by the IME | indicator binary is portable; IMK supplies the coords |
| App identity | `desktop_integration.rs` (`.desktop` + X11 `WM_CLASS`) | `cfg` out; replaced by `.app` `Info.plist` |
| Service lifecycle | systemd unit | launchd agent plist |
| Base directories | XDG (`XdgBaseDirs::Default`) | `~/Library/...` branch |

## The IMK component — implementation language

**Decision: implement in Rust via `objc2`, with Swift as the documented
fallback.**

We can subclass `IMKInputController` from Rust: `objc2`'s `define_class!` macro
subclasses framework classes and overrides methods, and the
`objc2-input-method-kit` bindings cover the IMK surface. The reason to prefer
Rust here is **reuse**, which is also the better TDD story:

- The IMK component **links `idiolect-ipc` directly** — the wire protocol
  (`messages.rs` / `framing.rs` / `handshake.rs`) is reused verbatim instead of
  re-derived as Swift `Codable` structs that could silently drift from the Rust
  enum.
- The IME **state machine** — today duplicated across the Rust IBus engine and
  the C++ `fcitx5/idiolect-fcitx5/src/engine.cpp` — collapses into **one shared
  Rust crate**, unit-tested under `cargo test` on the macOS runner. That is a
  stronger TDD position than XCTest, and it matches the repo's Rust-first
  master plan.

The genuine cost (and the reason Swift is the fallback, not the default):
IMK-from-Rust is lightly trodden. The fragile, example-poor parts are:

1. **Bundle/principal-class wiring** — `Info.plist`'s `NSPrincipalClass` /
   `InputMethodServerControllerClass` must resolve at load to the ObjC class we
   register from Rust.
2. **`IMKServer` + run loop** — `IMKServer initWithName:bundleIdentifier:`
   driven by an `NSApplication` run loop started from a Rust `main`.
3. **Debugging** — ObjC-runtime / IMK introspection failures are debugged
   through `objc2` with few reference projects (Swift has rime/Squirrel,
   fcitx5-macos).

If any of (1)–(3) proves intractable, fall back to a thin Swift IMK shell that
still talks to the daemon over the existing socket protocol — the rest of the
plan is unchanged.

### Behaviour parity with fcitx5

The controller mirrors `engine.cpp` exactly:

- `toggle()` — send `ToggleRecording`; the daemon alone decides start vs stop.
- Mirror `RecordingStatus` pushes; never flip phase locally.
- On take-final transcript → `insertText:replacementRange:` then
  `CommitPreedit` to finalise the training candidate.
- On partial (streaming) snippet → live `setMarkedText:...` (the underlined
  preedit), no commit; the daemon merges and finalises the take at stop.
- `cancel` / `on_error` → clear marked text, return to Idle.
- `PreeditUpdate{review:true}`, `InsertText`, `EditHistory` drive the existing
  egui dialogs, identical to the Linux front-end.

### Hotkey

Parity with fcitx5 first: handle the toggle in `handleEvent:` while Idiolect is
the active input source (documented "select Idiolect input source" step, the
analogue of "switch from ibus to fcitx5"). A global Carbon
`RegisterEventHotKey` (works regardless of active input source, needs
Accessibility/Input-Monitoring TCC grants) is a follow-up — see `003-global-hotkey`.

## Tray — why a separate agent

`NSStatusItem` requires the main thread plus an `NSApplication` run loop, which
conflicts with the headless tokio daemon (memory: "tray is part of daemon" on
Linux). On macOS the tray becomes a small `LSUIElement` menu-bar **agent
process** that subscribes to `RecordingStatus` and drives history actions
(`HistoryReinsert` / `HistoryCopy` / `EditHistory`) over the same socket —
consistent with the GUI helpers already being separate processes. This is an
intentional divergence from Linux's in-daemon ksni.

## Crate / tree changes

| Path | Change |
|---|---|
| `idiolect-common` | macOS branch in `XdgBaseDirs` + a socket-path length guard (`sun_path` ≤ ~104 bytes — `~/Library/Application Support/...` overflows) |
| `idiolect-adapter-whisper` | `metal = ["whisper-rs/metal"]` (+ optional `coreml`), threaded through `idiolectd` features |
| GUI helper crates | cfg the `eframe`/`egui-winit` feature set to Cocoa off-Linux |
| `idiolect-ime-core` (new) | shared Rust IME state machine, replacing the IBus/fcitx5 duplication |
| `macos/IdiolectIMK/` (new) | `objc2` IMK component + `.app` bundle (`Info.plist`, ad-hoc/Developer-ID signing, `TISRegisterInputSource`) |
| `idiolect-adapters/desktop/tray-macos` (new) | `NSStatusItem` menu-bar agent over IPC |
| `idiolectd` | `cfg(target_os)` adapter selection; `cfg` out `desktop_integration.rs` on macOS |
| `idiolect-cli` (+ ibus client `default_socket_path`) | route socket discovery through `XdgBaseDirs::for_platform`/`resolve_xdg_paths` instead of the hardcoded Linux `~/.local/run/idiolect` chain, so clients find the daemon's `$TMPDIR` socket on macOS. Currently Linux-only and unchanged; **must** land when `idiolect-cli` joins the macOS CI matrix (Phase 2), or `doctor`/history-reinsert will report the socket down while the daemon is up. |
| `dist/macos/` (new) | launchd plist, bundle-assembly + install scripts, TCC docs |

## TDD strategy

The repo is strictly TDD; every behaviour is covered at unit / integration /
e2e unless a level is a genuine GUI/desktop boundary (stated in the test file).

- **Rust (paths, socket-length guard, `metal` wiring, adapter selection, shared
  IME state machine, IPC codec)** — unit + integration tests under `cargo test`,
  runnable on the macOS CI runner. The wire protocol is already tested; reuse.
- **IMK ↔ system boundary** (real marked text in a live app, input-source
  registration, run-loop bring-up) — no headless seam; covered by the manual
  check stated in the test file, exactly as the repo documents other GUI/desktop
  boundaries.
- **Cross-process e2e** — real daemon socket ↔ the IMK component's IPC client,
  asserting handshake + a toggle→transcript→commit round-trip (the analogue of
  fcitx5's `e2e_ipc_bridge_test`).

## Phasing (each red → green; `cargo test --workspace` + `clippy` stay green)

1. **Foundation** — macOS `XdgBaseDirs` branch, socket-path length guard, macOS
   CI runner building the portable crates + running unit/integration tests.
2. **ASR + GUI on macOS** — `metal` feature, eframe Cocoa features, verify
   whisper inference + all four egui windows build and run.
3. **Shared IME state machine** — extract `idiolect-ime-core` from the
   IBus/fcitx5 duplication; full cargo coverage.
4. **IMK component** — `objc2` controller, IPC client, marked-text wiring,
   then the cross-process e2e. (Swift fallback if blocked.)
5. **Menu-bar tray agent** — `NSStatusItem` over IPC.
6. **Packaging** — launchd plist, bundle assembly + signing, install scripts,
   TCC grant docs, README macOS section + Linux↔macOS parity matrix.

## Release pipeline

`release-main.yml` builds a rolling `edge` prerelease off `main`. When phases
2/4/5 land, add a macOS build job (matrix `runs-on: macos-latest`) that compiles
the daemon/helpers with `--features metal`, builds the IMK `.app` + menu-bar
agent bundles, and uploads a `.dmg`/`.zip` alongside the Linux `.deb`. Until
then the macOS job is intentionally absent rather than perpetually red.

## Why not now

It is a large, multi-component effort touching a second OS toolchain (bundles,
signing, TCC, launchd) and an example-poor Rust↔IMK path. It should land as its
own milestone after the in-flight Burn LoRA training work, phase by phase, each
keeping both gates green.
