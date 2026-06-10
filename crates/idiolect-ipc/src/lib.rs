//! IPC protocol DTOs and JSON Lines framing for Idiolect clients.

pub mod framing;
pub mod handshake;
pub mod messages;

pub use messages::{
    ClientHello, CommitPreedit, ErrorMessage, HistoryCopy, HistoryCopyResponse, HistoryReinsert,
    HistoryReinsertResponse, InsertText, IpcMessage, PreeditUpdate, RecordingStatus,
    ReportCorrection, ServerHello,
};

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
