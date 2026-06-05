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
    pub kind: TrayMenuItemKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrayMenuItemKind {
    /// A plain clickable item, optionally with a nested submenu.
    Standard { submenu: Option<Vec<TrayMenuItem>> },
    /// A checkable toggle item (e.g. "Mute").
    Checkable { checked: bool },
    /// A mutually-exclusive radio group rendered as sibling items.
    /// The adapter expands this into individual radio items at render time.
    RadioGroup { options: Vec<String>, selected: usize },
    /// A visual separator.
    Separator,
}
```

The `RadioGroup` variant maps to `ksni::menu::RadioGroup` for the Settings submenu (retention days and max entries). The adapter's `fn menu()` expands each `RadioGroup` item into the appropriate `ksni::MenuItem::RadioGroup`.

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

### Migration `0004_text_history.sql`

> **Note:** migration 0003 (`0003_v1_storage.sql`) already exists in the codebase. The history table is migration **0004**.

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
| `src/config.rs` | Add `HistoryConfig` struct with `retention_days` (default 1) and `max_entries` (default 10). Add validation rules. Add a free function `write_history_config(config_path: &Path, history: &HistoryConfig) -> Result<(), ConfigError>` that serialises the whole `IdiolectConfig` back to TOML. No new port trait needed — this is a simple file write. |

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
| `migrations/0004_text_history.sql` | **New file.** Schema above. |
| `src/migrations.rs` | Register migration version 4 with its checksum (compute with `sha256sum` after writing the file). |
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

The IPC message loop lives in `run_loop.rs`. History storage and tray refresh must happen there, not only in `runtime.rs`.

| File | Change |
|---|---|
| `src/runtime.rs` | On startup: open `KsniTray`, open history DB, prune old entries, spawn a background thread that calls `history_port.prune(retention_days)` every hour (using `std::thread::sleep`), set the initial tray menu. Pass a `std::sync::mpsc::Sender<DaemonCommand>` into the ksni callback closures so menu activations route back to the main thread. |
| `src/run_loop.rs` | Extend `RunLoopConfig` to carry a `history_sender: mpsc::Sender<DaemonCommand>` and `tray_sender: ...`. After `CommitPreedit`, call `history_port.store(session_id, text, Committed)` and send a `DaemonCommand::RefreshTrayMenu` through the channel. After `CancelPreedit`, same with `Cancelled`. Add a `DaemonCommand` enum with variants `RefreshTrayMenu`, `InsertHistory(i64)`, `CopyHistory(i64)`, `DeleteHistory(i64)`, `UpdateHistoryConfig(HistoryConfig)`. |
| `src/adapters.rs` | Wire `KsniTray`, `ArboardClipboard`, and the mpsc channel into the adapter profile for production builds. |

### `idiolect-cli`

| File | Change |
|---|---|
| `src/lib.rs` | Add `history list`, `history show <id>`, `history delete <id>`, `history prune` subcommands. Each opens the DB, constructs a `SqliteMetadataStore` implementing `HistoryPort`, and runs the query. |

### `Cargo.toml` (workspace root)

| Change |
|---|
| Add `"crates/idiolect-adapter-ksni"` to `[workspace] members`. |
| Add `"crates/idiolect-adapter-clipboard"` to `[workspace] members`. |

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
  → Each history entry is a submenu with three actions:
    "restart Traefik" →
      ├─ Insert  → InputMethodPort::commit_text(session_id, "restart Traefik")
      ├─ Copy    → ClipboardPort::set_text("restart Traefik")
      └─ Delete  → HistoryPort::delete(id) + tray refresh
  → User clicks "Insert"
    → ksni activate callback with history entry id + action=insert
    → HistoryUseCase::reinsert(id)
      → InputMethodPort::commit_text(session_id, "restart Traefik")
  → User clicks "Copy"
    → ksni activate callback sends DaemonCommand::CopyHistory(id)
    → HistoryUseCase::copy(id)
      → ClipboardPort::set_text("restart Traefik")
  → User clicks "Delete"
    → ksni activate callback sends DaemonCommand::DeleteHistory(id)
    → HistoryPort::delete(id)
    → tray_port.set_menu(MenuUseCase::get_menu(...))
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

use std::sync::mpsc;

use idiolect_ports::tray::{TrayIcon, TrayMenuItemKind, TrayMenuItem, TrayPort, TrayStatus};
use ksni::menu::{StandardItem, SubMenu, RadioGroup};
use ksni::{Tray, TrayMethods};

/// Commands the tray adapter sends back to the daemon's main thread.
pub enum TrayCallback {
    /// Carry an opaque string action id (e.g. "insert:42", "copy:42", "delete:42").
    Activate(String),
}

pub struct KsniTray {
    handle: ksni::blocking::Handle<InnerTray>,
}

struct InnerTray {
    icon: String,
    tooltip: String,
    status: ksni::Status,
    menu_items: Vec<TrayMenuItem>,
    /// Channel used by ksni callbacks to notify the daemon main thread.
    sender: mpsc::Sender<TrayCallback>,
}

impl TrayPort for KsniTray {
    type Error = KsniTrayError;

    fn set_icon(&mut self, icon: TrayIcon) -> Result<(), Self::Error> { ... }
    fn set_tooltip(&mut self, tooltip: &str) -> Result<(), Self::Error> { ... }
    fn set_menu(&mut self, items: Vec<TrayMenuItem>) -> Result<(), Self::Error> { ... }
    fn set_status(&mut self, status: TrayStatus) -> Result<(), Self::Error> { ... }
}
```

`KsniTray::new` takes an `mpsc::Sender<TrayCallback>`. The `InnerTray` implements `ksni::Tray` and maps `TrayMenuItem` to `ksni::MenuItem` in its `fn menu()` method. Each leaf item's activate closure clones the sender and calls `sender.send(TrayCallback::Activate(action_id))`. `TrayMenuItemKind::RadioGroup` is expanded into a `ksni::menu::RadioGroup` containing one `RadioItem` per option. The daemon's main thread calls `mpsc::Receiver::try_recv` on each run-loop iteration to drain pending callbacks.

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
- History entries older than `history.retention_days` are pruned on daemon startup and every hour thereafter (background thread using `std::thread::sleep(Duration::from_secs(3600))`).
- Individual entries can be deleted via CLI (`idiolect history delete <id>`) or from the tray menu.

---

## Acceptance criteria

- [ ] `HistoryPort` trait in `idiolect-ports` with `store`, `recent`, `delete`, `prune`
- [ ] `TrayPort` trait in `idiolect-ports` with `set_icon`, `set_tooltip`, `set_menu`, `set_status`
- [ ] `SqliteMetadataStore` implements `HistoryPort`; migration `0004` applies cleanly on top of `0003`
- [ ] `KsniTray` in `idiolect-adapter-ksni` implements `TrayPort`
- [ ] `FakeHistoryStore`, `FakeTray`, and `FakeClipboard` in `idiolect-test-support`
- [ ] `MenuUseCase` returns correct items based on recording state
- [ ] `HistoryUseCase::get_recent`, `reinsert`, `copy`, and `delete` work through ports
- [ ] `ClipboardPort` trait in `idiolect-ports` with `set_text`
- [ ] `ArboardClipboard` in `idiolect-adapter-clipboard` implements `ClipboardPort`
- [ ] History submenu entries offer Insert, Copy, and Delete actions
- [ ] Daemon auto-stores history on commit/cancel
- [ ] Daemon updates tray icon and menu on commit/cancel/state changes
- [ ] Daemon prunes history on startup and every hour via background thread
- [ ] System tray icon appears on freedesktop desktop with correct right-click menu
- [ ] Clicking "Start Recording" in tray starts recording
- [ ] Clicking "Insert" on a history entry re-inserts text into the focused app
- [ ] Clicking "Copy" on a history entry copies text to system clipboard
- [ ] Clicking "Delete" on a history entry removes it and refreshes the menu
- [ ] Privacy export includes history count; privacy delete clears `ime_text_history`
- [ ] CLI `history list/show/delete/prune` commands work
- [ ] `HistoryConfig` in `IdiolectConfig` with `retention_days` (default 1) and `max_entries` (default 10)
- [ ] Settings submenu with radio groups for retention (1/7/30 days) and max entries (10/25/50)
- [ ] Selecting a settings option writes the new value to config and takes effect immediately
- [ ] ksni callbacks route to daemon main thread via `mpsc::Sender<TrayCallback>`
- [ ] Settings submenu uses `TrayMenuItemKind::RadioGroup`
- [ ] `write_history_config` persists updated `HistoryConfig` to XDG config file
- [ ] `idiolect-adapter-ksni` and `idiolect-adapter-clipboard` in workspace `Cargo.toml`
- [ ] No `ksni`, SQLite, or clipboard types leak into `idiolect-core`, `idiolect-ports`, or `idiolect-application`
- [ ] C++ shim has **no changes** for this feature

---

## Task breakdown for subagents

Each task below is self-contained enough to hand to a cheap single-purpose agent. List dependencies so tasks with none can run in parallel.

### Task 1 — Port traits (no dependencies)

**Crates:** `idiolect-ports`

Create three new files and update `lib.rs`:

- `src/history.rs` — `HistoryEntry`, `HistoryState`, `HistoryPort` trait exactly as specified in *Interface design* above.
- `src/tray.rs` — `TrayPort`, `TrayIcon`, `TrayStatus`, `TrayMenuItem`, `TrayMenuItemKind` exactly as specified above.
- `src/clipboard.rs` — `ClipboardPort` trait with `set_text`.
- `src/lib.rs` — add `pub mod history;`, `pub mod tray;`, `pub mod clipboard;`.

Deliverable: `cargo test -p idiolect-ports` passes (trait-compiles tests).

---

### Task 2 — `HistoryConfig` in `idiolect-common` (no dependencies)

**Crates:** `idiolect-common`

- Add `HistoryConfig { retention_days: u32, max_entries: u32 }` (defaults 1, 10) to `IdiolectConfig` in `src/config.rs` with `#[serde(default)]`.
- Add validation: `retention_days` in `{1, 7, 30}`, `max_entries` in `{10, 25, 50}`.
- Add `write_history_config(config_path: &Path, config: &IdiolectConfig) -> Result<(), ConfigError>` that serialises the full config to TOML and writes atomically (write to `.tmp`, rename).
- Unit-test round-trip: parse → serialise → parse, validate bounds rejection.

Deliverable: `cargo test -p idiolect-common` passes.

---

### Task 3 — SQLite migration + `HistoryPort` impl (depends on Task 1)

**Crates:** `idiolect-adapter-sqlite`

- Write `migrations/0004_text_history.sql` (schema from *Data model* above).
- Compute its SHA-256 with `sha256sum migrations/0004_text_history.sql` and register migration version 4 in `src/migrations.rs`.
- In `src/repository.rs`, implement `HistoryPort` for `SqliteMetadataStore`:
  - `store` — INSERT INTO `ime_text_history`.
  - `recent(limit)` — SELECT top N by `created_at DESC`.
  - `delete(id)` — DELETE by primary key.
  - `prune(older_than_days)` — DELETE WHERE `created_at < datetime('now', '-N days')`, return rows deleted.
- Update `delete_user_data` to also `DELETE FROM ime_text_history`.
- Update `privacy_export_summary` to include `{ "history_entries": <count> }`.

Unit tests:
- Migration 0004 applies on top of 0003.
- `store` + `recent` round-trip (reverse chronological order).
- `delete` removes only the target entry.
- `prune` removes entries older than N days, not newer ones.
- `delete_user_data` clears `ime_text_history`.

Deliverable: `cargo test -p idiolect-adapter-sqlite` passes.

---

### Task 4 — Test fakes (depends on Task 1)

**Crates:** `idiolect-test-support`

In `src/fakes.rs`, add:

- `FakeHistoryStore` — implements `HistoryPort` with an in-memory `Vec<HistoryEntry>`. `recent` returns last N in reverse insertion order. `delete` removes by id. `prune` removes where a fake `created_at` offset exceeds the threshold.
- `FakeTray` — implements `TrayPort`. Records all calls in a `Vec<String>` log. Exposes `icon()`, `tooltip()`, `menu_items()` accessors for assertions.
- `FakeClipboard` — implements `ClipboardPort`. Stores last text in `Option<String>`. Exposes `last_text()` accessor.

Deliverable: `cargo test -p idiolect-test-support` passes.

---

### Task 5 — `idiolect-adapter-ksni` new crate (depends on Task 1)

**Crates:** new `crates/idiolect-adapter-ksni/`

- `Cargo.toml`: depends on `idiolect-ports`, `ksni = { version = "0.3", features = ["blocking"] }`.
- `src/lib.rs`: `KsniTray` and `InnerTray` exactly as specified in *ksni adapter design* above.
  - `KsniTray::new(sender: mpsc::Sender<TrayCallback>) -> Result<Self, KsniTrayError>`
  - Implement `TrayPort for KsniTray`.
  - `InnerTray::menu()` maps `TrayMenuItem`/`TrayMenuItemKind` to `ksni::MenuItem`.
  - `RadioGroup` variant → `ksni::menu::RadioGroup`.
- Add the crate to workspace `Cargo.toml` `members`.

Deliverable: `cargo build -p idiolect-adapter-ksni` succeeds.

---

### Task 6 — `idiolect-adapter-clipboard` new crate (depends on Task 1)

**Crates:** new `crates/idiolect-adapter-clipboard/`

- `Cargo.toml`: depends on `idiolect-ports`, `arboard = "3"`.
- `src/lib.rs`: `ArboardClipboard` wrapping `arboard::Clipboard`. Implements `ClipboardPort`. `set_text` calls `arboard::Clipboard::set_text` and `arboard::Clipboard::clear_owned` so the text survives after the daemon process loses focus on X11/Wayland.
- Add the crate to workspace `Cargo.toml` `members`.

Deliverable: `cargo build -p idiolect-adapter-clipboard` succeeds.

---

### Task 7 — Application use cases (depends on Tasks 1, 2)

**Crates:** `idiolect-application`

- `src/use_cases/history.rs` — `HistoryUseCase<H, I, C>` with:
  - `get_recent(limit: u32) -> Result<Vec<HistoryEntry>, ...>`
  - `reinsert(id: i64) -> Result<(), ...>` — looks up entry, calls `InputMethodPort::commit_text`.
  - `copy(id: i64) -> Result<(), ...>` — looks up entry, calls `ClipboardPort::set_text`.
  - `delete(id: i64) -> Result<(), ...>` — delegates to `HistoryPort::delete`.
- `src/use_cases/menu.rs` — `MenuUseCase` (no generics, stateless):
  - `get_menu(recording_active: bool, history: &[HistoryEntry], config: &HistoryConfig) -> Vec<TrayMenuItem>`
  - Top-level items: Start/Stop Recording (one enabled based on state), Cancel, separator, Recent History submenu, Settings submenu.
  - History submenu: one `Standard { submenu: Some([Insert, Copy, Delete]) }` item per entry (label = entry text, truncated to 40 chars).
  - Settings submenu: two `RadioGroup` items — Retention and Max entries.
- `src/lib.rs` — add `pub mod history;` and `pub mod menu;` under `use_cases`.

Unit tests using `FakeHistoryStore`, `FakeInputMethod`, `FakeClipboard`, `FakeTray`:
- `get_menu` idle: Start Recording enabled, Stop disabled.
- `get_menu` recording: Stop enabled, Start disabled.
- `get_menu` with entries: history submenu has correct entries.
- `get_menu` Settings: RadioGroup options match config values.
- `reinsert` calls `commit_text` with correct text.
- `copy` calls `set_text` with correct text.
- `delete` removes entry from store.

Deliverable: `cargo test -p idiolect-application` passes.

---

### Task 8 — Daemon wiring (depends on Tasks 1–7)

**Crates:** `idiolectd`

- `src/runtime.rs`:
  - On production startup, create `KsniTray::new(sender)` and `ArboardClipboard::new()`.
  - Prune history on startup.
  - Spawn a background thread: `loop { thread::sleep(Duration::from_secs(3600)); history_port.prune(...); }`.
  - Set initial tray menu.
- `src/run_loop.rs`:
  - Extend `RunLoopConfig` with `tray_callback_rx: Option<mpsc::Receiver<TrayCallback>>`.
  - On each loop iteration call `try_recv` and dispatch `DaemonCommand` variants.
  - After `CommitPreedit`: call `history_port.store(..., Committed)`, refresh tray menu.
  - After `CancelPreedit`: call `history_port.store(..., Cancelled)`, refresh tray menu.
- `src/adapters.rs`:
  - Add `KsniTray` and `ArboardClipboard` to the production `RuntimeAdapterProfile`.
  - Add `FakeTray` and `FakeClipboard` to the fixture profile.

Deliverable: `cargo build -p idiolectd` succeeds.

---

### Task 9 — CLI `history` subcommands (depends on Tasks 1, 3)

**Crates:** `idiolect-cli`

In `src/lib.rs` `execute()` match, add:

```rust
[scope, action, rest @ ..] if scope == "history" && action == "list" => history_list(rest),
[scope, action, rest @ ..] if scope == "history" && action == "show" => history_show(rest),
[scope, action, rest @ ..] if scope == "history" && action == "delete" => history_delete(rest),
[scope, action, rest @ ..] if scope == "history" && action == "prune" => history_prune(rest),
```

Each function opens the DB via `SqliteMetadataStore::open_path`, calls `HistoryPort` methods, and returns JSON:
- `history list --json` — array of `{ id, session_id, text, state, created_at }`.
- `history show <id> --json` — single entry or `{ "code": "not-found" }`.
- `history delete <id> --json` — `{ "deleted": true }`.
- `history prune --json` — `{ "pruned": <count> }`.

All accept `--db <path>` flag (falling back to XDG default).

Deliverable: `cargo test -p idiolect-cli` passes including new subcommand tests.

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