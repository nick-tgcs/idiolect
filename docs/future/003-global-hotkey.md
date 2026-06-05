# 003 — Global Hotkey

**Status:** Future  
**Priority:** Medium  
**Effort:** Medium  

## Problem

Idiolect currently relies on Fcitx5's IME toggle to start/stop recording. This means the user must first switch to Idiolect as their input method, then activate it. MacWhisper offers a "global, access MacWhisper from anywhere" shortcut — a single keypress that starts recording regardless of which app is focused or which input method is active.

A global hotkey is more discoverable and faster than the IME toggle workflow.

## Proposal

Add a configurable global keybinding that starts/stops recording, independent of the input method framework.

### Config

```toml
[hotkey]
# Key binding in X11/evdev notation
binding = "Ctrl+Shift+Space"
# Whether the hotkey is enabled
enabled = true
```

### Implementation options

#### Option A: X11 keygrab (evdev)

Register a global key grab via X11. Works on X11 sessions. Does not work on Wayland (security model prevents global keygrabs).

```rust
// Uses x11rb or xcb crate
// GrabKey on the root window
// Forward to idiolectd via Unix socket
```

#### Option B: D-Bus shortcut (GNOME/KDE)

Register a shortcut via the desktop's D-Bus interface. Works on both X11 and Wayland but requires desktop-specific integration.

- GNOME: `org.gnome.SettingsDaemon.MediaKeys`
- KDE: `org.kde.kglobalaccel`

#### Option C: systemd-inhibit + evdev

Direct evdev device read (requires uinput permissions). Works on both X11 and Wayland but needs elevated permissions.

#### Option D: Fcitx5 trigger key

Use Fcitx5's existing trigger key infrastructure. This is the simplest but only works when Fcitx5 is running and Idiolect is configured as an input method.

### Recommended approach

Start with **Option D** (Fcitx5 trigger key) since it already works. Add **Option A** (X11 keygrab) as a fallback for non-Fcitx5 environments. Wayland users will need to configure their desktop's shortcut settings manually (documented in README).

### Data flow

```
User presses Ctrl+Shift+Space
  → X11 keygrab or Fcitx5 trigger
  → idiolectd receives StartRecording/StopRecording
  → If not recording: start recording, set tray icon to Recording
  → If recording: stop recording, commit/cancel preedit, set tray icon to Idle
```

### Tray menu

The hotkey binding is shown in the tray menu:

```
Settings →
  ├─ Remove fillers: ✓
  ├─ Retention: [● 1 day] [○ 7 days] [○ 30 days]
  ├─ Max entries: [● 10] [○ 25] [○ 50]
  └─ Hotkey: Ctrl+Shift+Space
```

Changing the hotkey from the tray menu is complex (requires key capture UI). For v1, the hotkey is configured in the TOML file only.

## Crate changes

| Crate | Change |
|---|---|
| `idiolect-common` | Add `HotkeyConfig` to `IdiolectConfig` |
| `idiolect-ports` | New `hotkey.rs` with `HotkeyPort` trait |
| `idiolect-adapter-x11` (new) | Implement `HotkeyPort` via X11 keygrab |
| `idiolectd` | Wire `HotkeyPort`, handle start/stop recording |

## Why not v1

The Fcitx5 trigger key already provides a working activation mechanism. Global hotkey is a quality-of-life improvement that requires platform-specific code (X11 vs Wayland) and testing across desktop environments. It's better to ship the tray menu first and add hotkeys once the core workflow is solid.