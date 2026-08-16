//! Idiolect IBus input-method engine.
//!
//! Layered so the dictation/correction logic is testable without IBus or a
//! display:
//! - [`session`]: the pure state machine (toggle, editable preedit, commit/cancel).
//! - [`ipc`]: the daemon IPC client (reuses the `idiolect-ipc` wire protocol).
//! - `ibus` (feature `ibus-engine`): the zbus glue that exposes the engine on
//!   the IBus bus.

pub mod focus;
pub mod helpers;
pub mod indicator;
pub mod ipc;
pub mod notify;
pub mod review;
pub mod session;

#[cfg(feature = "ibus-engine")]
pub mod ibus;
