//! Returning keyboard focus to the application after an auxiliary window (the
//! review dialog) has necessarily stolen it.
//!
//! The review dialog is a separate top-level window, so to let the user edit it
//! must take X11 focus away from the app they were typing into. On X11 the window
//! manager does not reliably hand focus back to the *exact* window afterwards, so
//! we capture which window was active before the dialog and re-assert focus on it
//! after — then the commit lands where the user is, and Enter works.
//!
//! Behind a trait so the engine logic and tests need no X server; the real
//! implementation (X11, `_NET_ACTIVE_WINDOW`) is compiled only with the engine.

/// An opaque handle to a top-level window (an X11 window id under the hood).
pub type WindowId = u32;

/// Captures the focused window and restores focus to it. A popup that must steal
/// focus uses this to hand it back to where the user was typing.
pub trait WindowFocus: Send + Sync {
    /// The currently active/focused top-level window, if discoverable.
    fn active_window(&self) -> Option<WindowId>;
    /// Re-assert focus on a previously captured window.
    fn restore(&self, window: WindowId);
}

/// No-op manager: capturing returns `None` (so `restore` is never called). Used
/// in tests and whenever no display/X11 is available.
pub struct NoopWindowFocus;

impl WindowFocus for NoopWindowFocus {
    fn active_window(&self) -> Option<WindowId> {
        None
    }
    fn restore(&self, _window: WindowId) {}
}

/// The default focus manager for the running engine: X11 when it can connect,
/// otherwise a no-op (e.g. on Wayland-only or headless sessions).
#[cfg(feature = "ibus-engine")]
#[must_use]
pub fn default_window_focus() -> Box<dyn WindowFocus> {
    match x11::X11WindowFocus::open() {
        Some(focus) => Box::new(focus),
        None => Box::new(NoopWindowFocus),
    }
}

/// Without the engine feature there is no X11 dependency, so focus management is
/// a no-op (the library + its tests build everywhere).
#[cfg(not(feature = "ibus-engine"))]
#[must_use]
pub fn default_window_focus() -> Box<dyn WindowFocus> {
    Box::new(NoopWindowFocus)
}

#[cfg(feature = "ibus-engine")]
mod x11 {
    use super::{WindowFocus, WindowId};

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        Atom, AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, Window,
    };
    use x11rb::rust_connection::RustConnection;

    /// Activates the user's window via the EWMH `_NET_ACTIVE_WINDOW` request,
    /// which a cooperating window manager (Mutter/GNOME, KWin, …) honours by
    /// raising and focusing it — the reparenting-safe way to restore focus.
    pub(super) struct X11WindowFocus {
        conn: RustConnection,
        root: Window,
        net_active_window: Atom,
    }

    impl X11WindowFocus {
        pub(super) fn open() -> Option<Self> {
            let (conn, screen_num) = x11rb::connect(None).ok()?;
            let root = conn.setup().roots.get(screen_num)?.root;
            let net_active_window = conn
                .intern_atom(false, b"_NET_ACTIVE_WINDOW")
                .ok()?
                .reply()
                .ok()?
                .atom;
            Some(Self {
                conn,
                root,
                net_active_window,
            })
        }
    }

    impl WindowFocus for X11WindowFocus {
        fn active_window(&self) -> Option<WindowId> {
            let reply = self
                .conn
                .get_property(false, self.root, self.net_active_window, AtomEnum::WINDOW, 0, 1)
                .ok()?
                .reply()
                .ok()?;
            let window = reply.value32()?.next()?;
            (window != 0).then_some(window)
        }

        fn restore(&self, window: WindowId) {
            // _NET_ACTIVE_WINDOW: data = [source, timestamp, requestor_active, 0, 0].
            // source = 2 ("pager") tells the WM to activate unconditionally,
            // bypassing focus-stealing prevention — we *are* deliberately handing
            // focus back to where the user was.
            let data = [2u32, x11rb::CURRENT_TIME, 0, 0, 0];
            let event = ClientMessageEvent::new(32, window, self.net_active_window, data);
            let _ = self.conn.send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            );
            let _ = self.conn.flush();
        }
    }
}
