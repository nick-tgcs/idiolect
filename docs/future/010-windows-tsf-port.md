# 010 — Windows Support (TSF IME Port)

**Status:** Future
**Priority:** Medium
**Effort:** Very High (multi-month)

> Sibling effort: [009 — macOS Port](009-macos-port.md). Both ports share the
> same starting point — the workspace has **zero `cfg(target_os)` guards** and a
> clean ports/adapters boundary — and overlap on the front-end refactor (lifting
> the platform-neutral engine out of the IBus crate) described below.

## Problem

Idiolect is Linux-only. The dictation pipeline (audio → VAD → ASR → state →
storage → training) is platform-neutral, but the *delivery* of text into the
focused application is done through an IBus engine, and the surrounding
desktop integration (tray, autostart, recording overlay, dock icon) is all
freedesktop/X11/GNOME-specific. To run on Windows we need a Windows-native way
to type into applications plus Windows equivalents of those surfaces.

The good news: the codebase is hexagonal (`idiolect-ports` traits + adapters)
and carries **no `#[cfg(target_os)]` in the core**. The Linux-ness is quarantined
in a handful of adapter crates. So this is "write Windows adapters behind
existing ports", not a rewrite.

## Decision: full TSF IME, broker + thin TIP

Windows' analogue to IBus is the **Text Services Framework (TSF)**. A TSF
*Text Input Processor* (TIP) is a COM in-proc DLL the OS loads into **every**
focused application's process. We commit to a real TSF TIP (not `SendInput`
keystroke synthesis) so we keep the live preedit/composition and in-place
correction experience, and so we can type into **elevated/admin windows** —
the TIP inherits the host app's integrity level, which a medium-IL `SendInput`
injector cannot do (UIPI blocks it).

Because a TIP is a per-app DLL with no long-lived process of its own, the
faithful design is **broker + thin TIP**:

- A persistent **broker** process owns the single daemon IPC connection, the
  `Session` state machine, the global toggle hotkey, the recording overlay,
  the tray, and the review dialog. This is the direct analogue of today's IBus
  engine *process*.
- The **TIP DLLs** are dumb text surfaces: they report focus/keys to the
  broker and execute broker-issued composition/commit operations against the
  host app's edit context.

This mirrors the existing daemon↔engine split exactly (so `session.rs` is
reused verbatim), keeps the daemon a single-client server (minimal daemon
change), and gives the global hotkey a natural owner. It is also how
Microsoft's own IMEs are structured (TIP DLL + manager process). The rejected
alternative — each TIP DLL running its own `Session` and daemon connection —
forces foreground-routing into the daemon, scatters take-state across
processes, and leaves the global hotkey homeless.

## Process topology

| Process | Role | Linux analogue |
|---|---|---|
| `idiolectd.exe` | mic + VAD + ASR + state + storage + review pipeline; IPC **server** | the daemon (systemd-run) |
| `idiolect-tsf-broker.exe` | persistent front-end: daemon IPC **client**, owns `Session`, global hotkey, recording overlay, tray, review dialog, foreground-TIP routing | the IBus engine process |
| `idiolect_tsf_tip.dll` | TSF TIP loaded into every focused app; reports focus/keys to broker, applies composition/commit | `ibus.rs` zbus glue |

Both `.exe`s launch at **user logon** (interactive session), **not** as a
SYSTEM service: they need the user's audio session, desktop, tray, overlay and
TSF, none of which a Session-0 service can do. The tray therefore moves from
the daemon into the broker on Windows (the daemon stays headless); the broker
already runs a message loop for the hotkey/overlay/TSF.

## What ports cleanly (the payoff of the hexagonal design)

- **`session.rs`** — the entire dictation/correction state machine (toggle,
  auto-commit, in-place keystroke correction, partial/streaming, review-dialog
  flow, focus-out). Zero platform deps. **Reused verbatim.**
- **Wire protocol** `idiolect-ipc` — newline-JSON, handshake v1, the
  `preedit`/`commit`/`recording_status` features. Transport-agnostic at the
  message layer.
- **`focus.rs`** — already a `WindowFocus` trait with a no-op default; just
  needs a Win32 impl.
- **Audio** (`cpal` → WASAPI), **ASR** (`whisper-rs`, incl. CUDA/cuBLAS),
  **clipboard** (`arboard`, already cross-platform), **GUI dialogs**
  (`eframe`/`egui` → win32 backend), sqlite, opus, burn-cuda trainer — all
  build on Windows.

## Linux-locked surfaces and their Windows replacements

| Concern | Linux today | Windows replacement |
|---|---|---|
| Text injection | IBus engine (zbus + x11rb) | TSF TIP (COM, `windows::Win32::UI::TextServices`) |
| IPC transport | `UnixListener`/`UnixStream` (`runtime.rs`, `run_loop.rs`, ibus `ipc.rs`) | Named pipe `\\.\pipe\idiolect-<sid>` behind a `Read+Write` seam |
| Config dirs | `XdgBaseDirs` (`idiolect-common/config.rs`) | `%APPDATA%`/`%LOCALAPPDATA%` via `directories` crate |
| Global trigger | compositor delivers Super+T to the engine | broker owns `RegisterHotKey` / `WH_KEYBOARD_LL` |
| Recording overlay | `_NET_WM_WINDOW_TYPE_NOTIFICATION` (eframe + x11/wayland) | `WS_EX_TOPMOST\|TOOLWINDOW\|NOACTIVATE` (+ `LAYERED\|TRANSPARENT` for click-through) |
| Tray | `ksni` (StatusNotifierItem over D-Bus) | `tray-icon` crate (`Shell_NotifyIcon`), hosted in the broker |
| Window/dock identity | `.desktop` + `gtk-update-icon-cache` (`desktop_integration.rs`) | embedded `.ico` + `SetCurrentProcessExplicitAppUserModelID` |
| Autostart | systemd unit | per-user Task Scheduler logon task / Startup shortcut |
| Focus restore | X11 `_NET_ACTIVE_WINDOW` (`focus.rs`) | `GetForegroundWindow`/`SetForegroundWindow` |

## Hard problems (called out, with mitigations)

1. **AppContainer / UWP & integrity boundaries (highest risk).** The TIP loads
   into sandboxed Store apps and elevated apps. The broker↔TIP pipe must be
   reachable from those: give the pipe a security descriptor granting
   `ALL APPLICATION PACKAGES` (capability SID `S-1-15-2-1`) for AppContainer,
   and rely on the TIP's in-process integrity for elevated apps. Small
   cross-instance state can ride TSF **global compartments**
   (`ITfCompartmentMgr`) instead of the pipe. Fallback if a sandbox blocks the
   pipe: composition-only-from-broker via `SendInput` for that one app,
   **logged, not silent**.
2. **Synchronous key-sink decisions.** `ITfKeyEventSink::OnTestKeyDown` must
   decide synchronously whether to eat a key — no round-trip. So the broker
   pushes the current `Session::State` (+ consume policy) to the foreground TIP
   on every change; the TIP decides locally. This matches `session.rs`: only
   `Trigger`/`Cancel` while `Recording` are consumed (return `true`); the
   correction-window keys return `false` (mirror **and** pass through).
3. **Live preedit vs. "commit partials directly".** Today `session.rs` commits
   partial snippets straight into the app (`Surface::commit_text`); there is no
   uncommitted composition. A real TSF IME wants an underlined composition for
   streaming partials, committed atomically at stop. That means widening the
   `Surface` trait with `set_composition`/`commit`/`clear` (default impls
   falling back to `commit_text`, so **Linux/IBus behavior is unchanged**) and
   teaching `on_partial_transcript`/`on_transcript` to prefer composition when
   available. Its own red→green sub-project.
4. **Focus capture/restore for the review dialog.** A Win32 `focus.rs` impl
   behind the existing `WindowFocus` trait.

## Crate changes

Lift the platform-neutral front-end out of the IBus crate so both front-ends
share it and only framework glue stays per-platform (this refactor is shared
with the [macOS port](009-macos-port.md)):

```
crates/idiolect-adapters/desktop/
  ime-core/        # NEW: session.rs + WindowFocus trait + ipc wire-client (no glue)
  ibus/            # zbus glue only (feature ibus-engine) — Linux leaf
  tsf/             # NEW: tip.rs (feature tsf-engine), broker.rs, win32 focus — Windows leaf
```

| Crate | Change |
|---|---|
| `idiolect-adapters/desktop/ime-core` (new) | Move `session.rs`, `focus.rs` trait, `ipc.rs` wire-client here (pure move; tests travel with it) |
| `idiolect-adapters/desktop/tsf` (new) | TSF TIP (`tsf-engine` feature), broker binary, Win32 `WindowFocus` impl |
| `idiolect-ipc` | Add a `Transport`/`Listener` seam over `Read+Write`; named-pipe impl |
| `idiolect-common` | Windows constructor for `XdgBaseDirs`; `directories`-backed dirs; pipe path |
| `idiolectd` | Bind the listener through the transport seam; `desktop_integration` becomes a cfg'd AppUserModelID no-op on Windows; tray hosted by broker |
| `idiolect-recording-indicator` | Win32 extended-style overlay path (cfg'd) |
| (workspace) | New deps: `windows` (TSF/Win32), `tray-icon`, `directories`, optional `interprocess` |

## Testing (the repo is strict TDD — three levels)

- **Unit** — `session.rs` tests (reused as-is); pure-logic tests for the
  named-pipe path/SDDL builder, the overlay extended-style computation
  (`fn → style flags`), AppUserModelID derivation, the transport seam against
  an in-memory pipe, and the broker's foreground-TIP routing/consume-policy
  table.
- **Integration** — broker↔daemon over a real named pipe (mirror the existing
  IBus integration test over a private socket); broker↔TIP over the local
  channel with a fake TIP.
- **E2E** — a gated `tsf_engine_e2e` (analogue of `ibus_engine_e2e`): register
  the TIP per-user, activate it against a hidden edit control via
  `ITfThreadMgr`, dictate a fixture, assert the control's text and a reported
  correction. `#[ignore]`-gated and Windows-only; state the GUI/desktop-boundary
  exception in the test file per CONTRIBUTING. COM glue stays behind a trait
  (as `ibus.rs` is feature-gated) so all logic is testable without a live TSF
  stack.
- **CI** — add a `windows-latest` runner (MSVC + CUDA toolkit + cmake for
  whisper/burn). Never `--all-features` (CUDA); keep tray-disabled/headless for
  non-gated runs; honor the coverage-map gate the same way the existing gated
  e2e does. The release pipelines (`release.yml` for tags, `release-main.yml`
  for the rolling `edge` build) gain a Windows build job once the port lands.

## Delivery sequence (full port — not a reduced scope)

1. **Refactor:** extract `ime-core` (pure move; both gates stay green on Linux).
2. **Transport seam:** abstract daemon listener + engine client over `Read+Write`;
   Unix impl unchanged, named-pipe impl + Windows `XdgBaseDirs`/pipe path. Daemon
   builds and serves on Windows.
3. **Broker skeleton:** persistent process, daemon IPC client, owns `Session`,
   global hotkey → toggle; no TIP yet (logs intended commits). Proves the
   audio→ASR→`Session` loop on Windows headlessly.
4. **TSF TIP:** COM server, per-user registration, focus/key sinks, edit-session
   commit; broker↔TIP channel with the AppContainer SDDL. Real text into apps.
5. **Composition/preedit:** widen `Surface`, live underlined streaming
   composition, atomic commit at stop.
6. **Surfaces:** Win32 focus impl + review dialog; recording overlay extended
   styles; tray in broker; AppUserModelID/icon.
7. **Autostart + packaging:** logon task; installer (MSIX preferred for clean
   per-user TIP registration and sandbox-friendliness, else MSI/WiX); CUDA/MSVC
   build docs.
8. **E2E + CI:** `tsf_engine_e2e`, Windows runner, parity pass against the Linux
   behavior matrix.

## Open questions

- **Packaging:** MSIX (clean per-user TIP registration, sandbox-friendly) vs.
  MSI/WiX (simpler CUDA bundling).
- **CUDA on Windows:** ship CUDA whisper/burn (large, driver-dependent) or
  default to CPU with CUDA opt-in? Affects build matrix and installer size.
- **Hotkey:** `RegisterHotKey` (simple, fixed) vs. `WH_KEYBOARD_LL` (needed for
  a Super-based combo, since Win+key combos are partly reserved).

## Why not now

It is a multi-month effort dominated by COM/TSF work, and Linux is the current
target platform. The hexagonal boundary means almost none of it touches core
logic — it is concentrated in the new `tsf` leaf crate plus the transport and
config seams — so it can be picked up later without disturbing the Linux build.
Relatedly, the global-hotkey work in [003](003-global-hotkey.md) overlaps with
the broker's hotkey ownership here, and the front-end `ime-core` extraction is
shared with the [macOS port](009-macos-port.md).
