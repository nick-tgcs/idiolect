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
5. All new storage goes through a new `HistoryPort` trait. No SQLite types leak into ports or application.
6. The `ksni` tray is an adapter behind a `TrayPort` trait, keeping the daemon's composition root as the only place that knows about `ksni`.

## Consequences

- Users get a system tray icon with a right-click menu — no hotkey memorization needed.
- Users can recover missed transcriptions from the history submenu.
- The C++ shim does **not** grow — menu rendering stays in Rust.
- New IPC messages are **not needed for the menu** — the tray lives in the same process as the daemon.
- One new SQLite migration (`0003_text_history.sql`).
- New `HistoryPort` trait in `idiolect-ports` isolates history queries from session metadata. `MetadataStorePort` is unchanged.
- New `TrayPort` trait in `idiolect-ports` abstracts the system tray, with `ksni` as the adapter.
- `InputMethodPort` is unchanged — history re-insertion reuses `commit_text`.
- Privacy export/delete covers `ime_text_history`.