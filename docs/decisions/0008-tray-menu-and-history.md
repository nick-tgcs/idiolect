# Decision 0008: System Tray Menu and Text History

Status: Proposed

## Context

Idiolect currently provides a single interaction path: press a key → record audio → receive preedit text → edit → commit. There is no way for the user to:

1. **Access a menu** of actions (start/stop recording, view status) without remembering hotkeys.
2. **Retrieve past transcriptions** that were committed or cancelled, in case the user missed or lost the text.

Speech-to-text is ephemeral, and users need a way to recover or review what was dictated.

## Decision

### 1. System Tray Menu via ksni

Use the `ksni` crate (freedesktop StatusNotifierItem / system tray) to provide the action menu directly from the Rust daemon:

- **Desktop-native** — appears as a system tray icon in KDE, GNOME (with AppIndicator), and other freedesktop-compliant environments.
- **Pure Rust** — no C++ changes needed for the menu itself.
- **Framework-agnostic** — works with any input method framework.

Menu items:

| Menu Item | Action |
|---|---|
| **Start Recording** | Begin audio capture and STT |
| **Stop Recording** | End current dictation session |
| **Cancel** | Discard current preedit |
| **Status** | Show daemon status (model loaded, recording state) |
| **Recent History →** | Submenu listing last N transcriptions |
| **Settings →** | Submenu with retention (1/7/30 days) and max entries (10/25/50) radio groups |

### 2. Text History

A history of recent text snippets stored automatically on every commit and cancel. The user can select a past snippet from the "Recent History" submenu to re-insert it into the focused application. History entries are retained for a configurable period (default 1 day, options: 1, 7, 30 days) and a configurable maximum count (default 10, options: 10, 25, 50). Both are adjustable from the tray menu.

### 3. Design Rules

1. The menu is a **system tray menu** rendered by `ksni` in the Rust daemon.
2. The C++ engine shim stays thin — it only handles preedit/commit/cancel IPC as before.
3. History is populated automatically on every commit and cancel.
4. History selection reuses the existing `commit_text` path — no new `InputMethodPort` methods needed.
5. **History queries are a read projection of session data.** Extend `MetadataStorePort` (in `idiolect-ports`) with `recent_history(limit)` and `prune_history(older_than_days)` methods. No new `HistoryPort` trait.
6. **Desktop integration (tray, clipboard) lives in the adapter layer**, not in `idiolect-ports`. The `ksni` tray adapter and `arboard` clipboard adapter live in `crates/idiolect-adapters/desktop/`. The daemon (`idiolectd`) wires them at the composition root.
7. `InputMethodPort` is unchanged — history re-insertion reuses `commit_text`.

## Consequences

- Users get a system tray icon with a right-click menu — no hotkey memorization needed.
- Users can recover missed transcriptions from the history submenu.
- The C++ shim does **not** grow — menu rendering stays in Rust.
- New IPC messages are **not needed for the menu** — the tray lives in the same process as the daemon.
- One new SQLite migration (`0004_text_history.sql` — after existing `0003_v1_storage.sql`).
- `MetadataStorePort` extended with history query methods; no new port trait.
- Desktop integration adapters (`ksni` tray, `arboard` clipboard) in `idiolect-adapters/desktop/`, wired by `idiolectd`.
- `InputMethodPort` is unchanged — history re-insertion reuses `commit_text`.
- Privacy export/delete covers `ime_text_history`.