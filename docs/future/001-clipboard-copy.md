# 001 — Clipboard Copy

**Status:** Planned for current tray & history implementation  
**Priority:** High  
**Effort:** Small  

## Problem

When a user selects a history entry from the tray menu, the current plan only re-inserts text via `InputMethodPort::commit_text`. This works when the user wants to type into the currently focused app, but fails when they want to paste into a different app, a web form, a terminal, or any context where IME commit isn't appropriate.

Every major transcription app (MacWhisper, Speechnotes) offers both "insert" and "copy" actions for transcript text.

## Proposal

Add a `ClipboardPort` trait and a second action on each history entry:

```rust
// crates/idiolect-ports/src/clipboard.rs

pub trait ClipboardPort {
    type Error;
    fn set_text(&mut self, text: &str) -> Result<(), Self::Error>;
}
```

Each history entry in the tray menu gets two actions:

- **Insert** — commits text into the focused app via `InputMethodPort::commit_text` (existing behavior)
- **Copy** — copies text to the system clipboard via `ClipboardPort::set_text`

The tray menu shows each history entry as a submenu:

```
Recent History →
  ├─ "restart Traefik" →
  │    ├─ Insert
  │    └─ Copy
  ├─ "hello world" →
  │    ├─ Insert
  │    └─ Copy
  └─ Clear History
```

## Adapter

`idiolect-adapter-clipboard` uses the `arboard` crate (cross-platform Rust clipboard access):

```toml
[dependencies]
arboard = "3"
```

On Wayland, `arboard` uses `wl-copy`/`wl-paste`. On X11, it uses X selections. No C++ dependency.

## Crate changes

| Crate | Change |
|---|---|
| `idiolect-ports` | New `clipboard.rs` with `ClipboardPort` trait |
| `idiolect-adapter-clipboard` | New crate implementing `ClipboardPort` via `arboard` |
| `idiolect-test-support` | `FakeClipboard` implementing `ClipboardPort` |
| `idiolect-application` | `HistoryUseCase` gains `copy(history_id)` method using `ClipboardPort` |
| `idiolectd` | Wire `ClipboardPort` into `DaemonState`, handle "copy" callback from tray |
| `Cargo.toml` | Add `idiolect-adapter-clipboard` workspace member |

## Privacy

No privacy concern — clipboard is ephemeral and not stored. The `ClipboardPort` only writes; it never reads.

## Why not just always copy?

IME commit is the primary action because it's seamless — the text appears as if the user typed it. Clipboard copy is the fallback for when the user wants to paste elsewhere. Offering both gives the user control.