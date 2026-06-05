# System Tray Menu & Text History — Implementation Plan

## What we are building

Three features:

1. **System tray menu** — a `ksni`-based tray icon in the Rust daemon providing a right-click menu with recording controls, status, and a history submenu. Works on any freedesktop-compliant desktop (KDE, GNOME with AppIndicator, etc.).
2. **Text history** — a list of recent committed and cancelled transcriptions shown as a submenu. Each entry offers both **Insert** (re-inserts into the focused app via `commit_text`) and **Copy** (copies to system clipboard).
3. **Clipboard copy** — a `ClipboardPort` abstraction so history entries can be copied to the system clipboard, not just re-inserted via IME. Uses the `arboard` crate for cross-platform clipboard access.

The menu lives entirely in the Rust daemon process. No C++ changes needed.

---

## Interface design

### New port: `HistoryPort`

History queries are a separate concern from session metadata. Rather than bloating `MetadataStorePort`, we introduce `HistoryPort` in `idiolect-ports`:

```rust
// crates/idiolect-ports/src/history.rs

use idiolect_common::ids::ImeSessionId;

pub struct HistoryEntry {
    pub id: i64,
    pub session_id: ImeSessionId,
    pub text: String,
    pub state: HistoryState,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoryState {
    Committed,
    Cancelled,
}

pub trait HistoryPort {
    type Error;

    fn store(&mut self, session_id: ImeSessionId, text: &str, state: HistoryState) -> Result<(), Self::Error>;
    fn recent(&self, limit: u32) -> Result<Vec<HistoryEntry>, Self::Error>;
    fn delete(&mut self, id: i64) -> Result<(), Self::Error>;
    fn prune(&mut self, older_than_days: u32) -> Result<u64, Self::Error>;
}
```

Why a separate port:

- `MetadataStorePort` owns session lifecycle (create, record edits, commit, cancel). History is a *read projection* of that lifecycle — a derived view, not a primary operation.
- A separate port lets us swap the history backend (in-memory LRU for tests, SQLite for production) without touching session storage.
- Single-responsibility boundary: `MetadataStorePort` = session truth, `HistoryPort` = recent-text query.

### New port: `TrayPort`

The system tray is abstracted behind a port so the daemon can be tested without a desktop:

```rust
// crates/idiolect-ports/src/tray.rs

pub trait TrayPort {
    type Error;

    fn set_icon(&mut self, icon: TrayIcon) -> Result<(), Self::Error>;
    fn set_tooltip(&mut self, tooltip: &str) -> Result<(), Self::Error>;
    fn set_menu(&mut self, items: Vec<TrayMenuItem>) -> Result<(), Self::Error>;
    fn set_status(&mut self, status: TrayStatus) -> Result<(), Self::Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrayIcon {
    Idle,
    Recording,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrayStatus {
    Active,
    Passive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayMenuItem {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub submenu: Option<Vec<TrayMenuItem>>,
    pub toggle: Option<bool>,       // for checkmark items
    pub radio_group: Option<String>, // items with the same group are mutually exclusive
    pub radio_selected: Option<usize>, // which option is selected in the group
}
```

The `radio_group` and `radio_selected` fields map to `ksni::menu::RadioGroup` for the Settings submenu (retention days and max entries).

The `ksni` adapter in `idiolect-adapter-ksni` implements `TrayPort` by mapping `TrayMenuItem` to `ksni::MenuItem`.

### `MetadataStorePort` stays unchanged

`commit_session` and `cancel_session` remain as they are. The daemon orchestrates: after a successful commit/cancel it also calls `HistoryPort::store`.

### New port: `ClipboardPort`

History entries need two actions: **Insert** (via `InputMethodPort::commit_text`) and **Copy** (via `ClipboardPort::set_text`). The clipboard port abstracts platform clipboard access so the daemon can be tested without a desktop:

```rust
// crates/idiolect-ports/src/clipboard.rs

pub trait ClipboardPort {
    type Error;
    fn set_text(&mut self, text: &str) -> Result<(), Self::Error>;
}
```

The `arboard` crate provides cross-platform clipboard access (X11 via X selections, Wayland via `wl-copy`/`wl-paste`).

### `InputMethodPort` stays unchanged

History re-insertion calls `commit_text` on the existing `InputMethodPort`. No new methods needed.

### New config section

```toml
[history]
retention_days = 1
max_entries = 10
```

Added to `IdiolectConfig` as `HistoryConfig` in `idiolect-common/src/config.rs`.

Defaults are intentionally small (1 day, 10 entries) because speech-to-text history is sensitive. The tray menu offers preset options:

| Setting | Options |
|---|---|
| Retention | 1 day (default), 7 days, 30 days |
| Max entries | 10 (default), 25, 50 |

Selecting a different option writes the new value to the config file and takes effect immediately.

---

## Data model

### Migration `0003_text_history.sql`

```sql
CREATE TABLE ime_text_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    text TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('committed', 'cancelled')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY(session_id) REFERENCES ime_text_sessions(id)
);

CREATE INDEX ime_text_history_created_at_lookup
    ON ime_text_history(created_at DESC);
CREATE INDEX ime_text_history_session_lookup
    ON ime_text_history(session_id);
```

Populated automatically by the daemon on commit/cancel. Pruned on startup and periodically.

---

## Crate-by-crate changes

### `idiolect-common`

| File | Change |
|---|---|
| `src/config.rs` | Add `HistoryConfig` struct with `retention_days` (default 1) and `max_entries` (default 10). Add validation. Add `ConfigPort` trait for runtime config writes, or a `write_config_value` method on `IdiolectConfig`. |

### `idiolect-ports`

| File | Change |
|---|---|
| `src/history.rs` | **New file.** `HistoryEntry`, `HistoryState`, `HistoryPort` trait. |
| `src/tray.rs` | **New file.** `TrayPort`, `TrayIcon`, `TrayStatus`, `TrayMenuItem`. |
| `src/clipboard.rs` | **New file.** `ClipboardPort` trait with `set_text` method. |
| `src/lib.rs` | Add `pub mod history;`, `pub mod tray;`, and `pub mod clipboard;`. |

### `idiolect-adapter-sqlite`

| File | Change |
|---|---|
| `migrations/0003_text_history.sql` | **New file.** Schema above. |
| `src/migrations.rs` | Register the new migration with its checksum. |
| `src/repository.rs` | Implement `HistoryPort` for `SqliteMetadataStore`. Add `store`, `recent`, `delete`, `prune` methods. Update `delete_user_data` to also `DELETE FROM ime_text_history`. Update `privacy_export_summary` to include history count. |

### `idiolect-adapter-ksni` (new crate)

| File | Change |
|---|---|
| `Cargo.toml` | **New file.** Depends on `idiolect-ports`, `ksni = "0.3"`. |
| `src/lib.rs` | **New file.** `KsniTray` struct implementing `TrayPort`. Maps `TrayIcon` → icon names, `TrayMenuItem` → `ksni::MenuItem`. Uses `ksni::blocking` feature to avoid async dependency in the daemon. |

### `idiolect-adapter-clipboard` (new crate)

| File | Change |
|---|---|
| `Cargo.toml` | **New file.** Depends on `idiolect-ports`, `arboard = "3"`. |
| `src/lib.rs` | **New file.** `ArboardClipboard` struct implementing `ClipboardPort`. Delegates to `arboard::Clipboard::set_text`. |

### `idiolect-test-support`

| File | Change |
|---|---|
| `src/fakes.rs` | Add `FakeHistoryStore` implementing `HistoryPort` with an in-memory `Vec<HistoryEntry>`. Add `FakeTray` implementing `TrayPort` that records calls. Add `FakeClipboard` implementing `ClipboardPort` that stores the last text set. |

### `idiolect-application`

| File | Change |
|---|---|
| `src/use_cases/history.rs` | **New file.** `HistoryUseCase<H: HistoryPort, I: InputMethodPort, C: ClipboardPort>` with `get_recent(limit)`, `reinsert(history_id)` (Insert action), and `copy(history_id)` (Copy action) methods. |
| `src/use_cases/menu.rs` | **New file.** `MenuUseCase` with `get_menu(recording_active: bool, history_config: &HistoryConfig) -> Vec<TrayMenuItem>`. Includes Settings submenu with radio groups for retention (1/7/30 days) and max entries (10/25/50). |
| `src/use_cases/dictation.rs` | No change. The daemon orchestrates history storage after commit/cancel. |
| `src/lib.rs` | Add `pub mod history;` and `pub mod menu;` to `use_cases`. |

### `idiolectd`

| File | Change |
|---|---|
| `src/runtime.rs` | Add `DaemonState` struct tracking `recording_active`, `session_id`, `current_text`. Wire `HistoryPort`, `TrayPort`, and `ClipboardPort`. After `CommitPreedit`/`CancelPreedit`, call `history_port.store(...)` and `tray_port.set_menu(...)`. On startup, create `KsniTray`, set initial menu, prune history. Handle tray callbacks for Insert (via `InputMethodPort`) and Copy (via `ClipboardPort`). |

### `idiolect-cli`

| File | Change |
|---|---|
| `src/lib.rs` | Add `history list`, `history show <id>`, `history delete <id>`, `history prune` subcommands. Each opens the DB, constructs a `SqliteMetadataStore` implementing `HistoryPort`, and runs the query. |

### C++ shim (`fcitx5/idiolect-fcitx5`)

**No changes.**

---

## Data flow

### Tray menu interaction

```
User right-clicks tray icon
  → ksni renders menu from TrayPort::set_menu items
  → User clicks "Start Recording"
    → ksni activate callback
    → idiolectd starts recording
    → tray_port.set_icon(TrayIcon::Recording)
    → tray_port.set_menu(MenuUseCase::get_menu(recording_active: true, &history_config))
```

### History submenu

```
User right-clicks tray icon → "Recent History →"
  → ksni renders submenu from TrayMenuItem with submenu items
  → Each history entry is a submenu with two actions:
    "restart Traefik" →
      ├─ Insert  → InputMethodPort::commit_text(session_id, "restart Traefik")
      └─ Copy    → ClipboardPort::set_text("restart Traefik")
  → User clicks "Insert"
    → ksni activate callback with history entry id + action=insert
    → HistoryUseCase::reinsert(id)
      → InputMethodPort::commit_text(session_id, "restart Traefik")
  → User clicks "Copy"
    → ksni activate callback with history entry id + action=copy
    → HistoryUseCase::copy(id)
      → ClipboardPort::set_text("restart Traefik")
```

### Settings submenu

```
User right-clicks tray icon → "Settings →"
  → ksni renders submenu with radio groups:
    Retention: [● 1 day] [○ 7 days] [○ 30 days]
    Max entries: [● 10] [○ 25] [○ 50]
  → User clicks "7 days"
    → ksni activate callback
    → idiolectd updates HistoryConfig.retention_days = 7
    → config is written to disk
    → tray_port.set_menu(MenuUseCase::get_menu(recording_active, &history_config))
    → history_port.prune(7) runs in background
```

### Auto-store on commit

```
User commits preedit "restart Traefik"
  → IpcClient::commit_preedit("restart Traefik")
    → idiolectd
      → DictationUseCase::commit(session_id, "restart Traefik", key)
      → HistoryPort::store(session_id, "restart Traefik", Committed)
      → TrayPort::set_menu(MenuUseCase::get_menu(recording_active: false, &history_config))
```

### Tray icon state

| Daemon state | Tray icon | Tooltip |
|---|---|---|
| Idle, waiting for connection | `idle` | "Idiolect — Ready" |
| Recording | `recording` | "Idiolect — Recording…" |
| Error (model not found, etc.) | `error` | "Idiolect — Error: …" |

---

## ksni adapter design

The `idiolect-adapter-ksni` crate implements `TrayPort` using `ksni`'s blocking API:

```rust
// crates/idiolect-adapter-ksni/src/lib.rs

use idiolect_ports::tray::{TrayIcon, TrayMenuItem, TrayPort, TrayStatus};
use ksni::menu::{StandardItem, SubMenu};
use ksni::{Tray, TrayMethods};

pub struct KsniTray {
    handle: ksni::blocking::Handle<InnerTray>,
}

struct InnerTray {
    icon: String,
    tooltip: String,
    status: ksni::Status,
    menu_items: Vec<TrayMenuItem>,
    callbacks: Vec<Box<dyn Fn() + Send + Sync>>,
}

impl TrayPort for KsniTray {
    type Error = KsniTrayError;

    fn set_icon(&mut self, icon: TrayIcon) -> Result<(), Self::Error> { ... }
    fn set_tooltip(&mut self, tooltip: &str) -> Result<(), Self::Error> { ... }
    fn set_menu(&mut self, items: Vec<TrayMenuItem>) -> Result<(), Self::Error> { ... }
    fn set_status(&mut self, status: TrayStatus) -> Result<(), Self::Error> { ... }
}
```

The `InnerTray` implements `ksni::Tray` and maps `TrayMenuItem` to `ksni::MenuItem` in its `fn menu()` method. Callbacks are wired through the daemon's event loop — when a menu item is activated, the daemon performs the corresponding action (start recording, reinsert history entry, etc.).

**Dependency:** `ksni = { version = "0.3", features = ["blocking"] }` — avoids async runtime dependency since the daemon uses synchronous Unix socket IPC.

---

## Tests

### Unit tests (per crate)

| Crate | Tests |
|---|---|
| `idiolect-ports` | `HistoryPort` trait compiles. `HistoryState` round-trips through serde. `TrayPort` trait compiles. `TrayMenuItem` round-trips through serde. |
| `idiolect-adapter-sqlite` | Migration `0003` applies on top of `0002`. `store` + `recent` round-trip. `recent` returns reverse chronological. `delete` removes entry. `prune` removes old entries. `delete_user_data` clears `ime_text_history`. |
| `idiolect-test-support` | `FakeHistoryStore` implements `HistoryPort`. `FakeTray` implements `TrayPort`. |
| `idiolect-application` | `MenuUseCase::get_menu` returns correct items for recording/not-recording states, including Settings submenu with radio groups reflecting current `HistoryConfig`. `HistoryUseCase::get_recent` delegates to port. `HistoryUseCase::reinsert` calls `commit_text`. |
| `idiolectd` | Fixture server: after `CommitPreedit`, `HistoryPort::store` is called with `Committed` state. After `CancelPreedit`, `HistoryPort::store` is called with `Cancelled` state. |

### Contract tests

| Port | Contract |
|---|---|
| `HistoryPort` | `FakeHistoryStore` and `SqliteMetadataStore` both satisfy the trait. Store → recent returns the entry. Delete → entry gone. Prune → old entries gone. |
| `TrayPort` | `FakeTray` and `KsniTray` both satisfy the trait. Set icon → icon updated. Set menu → menu items updated. |

### Integration test (daemon + tray)

- Start daemon with `FakeTray` and `FakeHistoryStore`.
- Simulate commit → verify `FakeTray` received updated menu with history entry.
- Simulate cancel → verify `FakeTray` received updated menu.
- Verify `FakeHistoryStore` has entries in reverse chronological order.

---

## Privacy

- `idiolect privacy export` includes `ime_text_history` count in the summary.
- `idiolect privacy delete` runs `DELETE FROM ime_text_history` inside the existing transaction.
- History entries older than `history.retention_days` are pruned on daemon startup and every hour thereafter.
- Individual entries can be deleted via CLI (`idiolect history delete <id>`) or from the tray menu.

---

## Acceptance criteria

- [ ] `HistoryPort` trait in `idiolect-ports` with `store`, `recent`, `delete`, `prune`
- [ ] `TrayPort` trait in `idiolect-ports` with `set_icon`, `set_tooltip`, `set_menu`, `set_status`
- [ ] `SqliteMetadataStore` implements `HistoryPort`; migration `0003` applies cleanly
- [ ] `KsniTray` in `idiolect-adapter-ksni` implements `TrayPort`
- [ ] `FakeHistoryStore` and `FakeTray` in `idiolect-test-support`
- [ ] `MenuUseCase` returns correct items based on recording state
- [ ] `HistoryUseCase::get_recent`, `reinsert`, and `copy` work through ports
- [ ] `ClipboardPort` trait in `idiolect-ports` with `set_text`
- [ ] `ArboardClipboard` in `idiolect-adapter-clipboard` implements `ClipboardPort`
- [ ] `FakeClipboard` in `idiolect-test-support`
- [ ] History submenu entries offer both Insert and Copy actions
- [ ] Clicking "Copy" on a history entry copies text to system clipboard
- [ ] Daemon auto-stores history on commit/cancel
- [ ] Daemon updates tray icon and menu on state changes
- [ ] Daemon prunes history on startup
- [ ] System tray icon appears on desktop with correct menu
- [ ] Clicking "Start Recording" in tray starts recording
- [ ] Clicking a history entry in tray re-inserts text
- [ ] Privacy export includes history count; privacy delete clears history
- [ ] CLI `history list/show/delete/prune` commands work
- [ ] `HistoryConfig` in `IdiolectConfig` with `retention_days` (default 1) and `max_entries` (default 10)
- [ ] Settings submenu in tray with radio groups for retention (1/7/30 days) and max entries (10/25/50)
- [ ] Selecting a settings option writes the new value to config and takes effect immediately
- [ ] No `ksni`, SQLite, or clipboard types leak into `idiolect-core`, `idiolect-ports`, or `idiolect-application`
- [ ] C++ shim has **no changes** for this feature

---

## Future features

Features researched from best-in-class transcription apps (MacWhisper, Speechnotes, whisper.cpp) that are **not** included in this plan but documented for future consideration:

| # | Feature | Priority | Doc |
|---|---|---|---|
| 002 | Filler word removal | Medium | `docs/future/002-filler-word-removal.md` |
| 003 | Global hotkey | Medium | `docs/future/003-global-hotkey.md` |
| 004 | Transcript search (FTS5) | Low | `docs/future/004-transcript-search.md` |
| 005 | Export formats (JSON, Markdown, SRT, VTT) | Low | `docs/future/005-export-formats.md` |
| 006 | Speaker diarization | Low | `docs/future/006-speaker-diarization.md` |
| 007 | Meeting recording (system audio capture) | Low | `docs/future/007-meeting-recording.md` |
| 008 | AI post-processing (spelling/grammar correction) | Low | `docs/future/008-ai-post-processing.md` |

Clipboard copy (001) is included in this plan.