//! Crate documentation for the Idiolect workspace.

pub mod daemon;
mod desktop_integration;
mod retention_dialog;
mod run_loop;
pub mod runtime;
mod settings_launcher;
mod sync_panel_launcher;

mod adapters;

/// The systemd unit journald files this daemon's stderr under. Helper-failure
/// notifications tell the user to grep this unit, so a rename here without a
/// rename in `packaging/` sends them to an empty journal — pinned by
/// `the_journal_unit_matches_the_packaged_service` below.
pub(crate) const DAEMON_UNIT: &str = "idiolectd";

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn the_journal_unit_matches_the_packaged_service() {
        // The notification body tells the user to run
        // `journalctl --user -u <DAEMON_UNIT>`; if that unit does not exist,
        // every helper-failure alert sends them somewhere empty.
        let unit = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/debian/usr/lib/systemd/user")
            .join(format!("{}.service", super::DAEMON_UNIT));

        assert!(
            unit.exists(),
            "no packaged unit named {}.service at {}",
            super::DAEMON_UNIT,
            unit.display()
        );
    }

    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
