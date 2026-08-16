//! Crate documentation for the Idiolect workspace.

pub mod daemon;
mod desktop_integration;
mod retention_dialog;
mod run_loop;
pub mod runtime;
mod settings_launcher;
mod sync_panel_launcher;

mod adapters;

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
