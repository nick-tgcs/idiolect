//! How the IBus engine tells the user its helpers failed.
//!
//! The engine's environment differs from the daemon's in two ways that matter:
//!
//! * It is launched by ibus-daemon from the component `<exec>`, with no
//!   arguments — so unlike the daemon, which systemd starts with an explicit
//!   `--config`, it has to resolve the config file itself. It uses the same
//!   resolver as the rest of the workspace ([`XdgBaseDirs`]), which is what
//!   `ResolvedConfigPaths::config_file` uses.
//! * **Its stderr is discarded.** ibus-daemon gives the engine `/dev/null`
//!   (which `ibus.rs` already relies on for its trace log), so writing a
//!   diagnostic to stderr records nothing at all. Engine-side failures go to a
//!   log file instead, and the notification points the user at that file rather
//!   than at a journal unit that will never contain it.

use std::path::{Path, PathBuf};

use idiolect_common::config::{DaemonConfig, IdiolectConfig, Platform, XdgBaseDirs};

/// The user's configured notify command.
#[must_use]
pub fn configured_notify_command() -> String {
    notify_command_from(&config_path())
}

/// Read the command out of a specific config file.
///
/// A missing or malformed config falls back to the packaged default rather
/// than to silence: a config the engine cannot parse is no reason to stop
/// telling the user their dictated take was thrown away. An explicitly EMPTY
/// value is honoured, because that is how the user turns notifications off.
#[must_use]
pub fn notify_command_from(config_path: &Path) -> String {
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|text| IdiolectConfig::from_toml_str(&text).ok())
        .map_or_else(
            || DaemonConfig::default().notify_command,
            |config| config.daemon.notify_command,
        )
}

/// Where the engine's helper-failure diagnostics are written, since its stderr
/// goes nowhere. The notification names this path.
#[must_use]
pub fn diagnostics_log_path() -> PathBuf {
    base_dirs().data_home.join("idiolect").join("engine.log")
}

/// The same path `ResolvedConfigPaths::config_file` computes, so the engine and
/// the rest of the workspace agree. (The packaged systemd unit hands the daemon
/// a hardcoded `%h/.config/idiolect/config.toml`, so a user who sets
/// `XDG_CONFIG_HOME` moves this file for the engine but not for that unit —
/// a pre-existing quirk of the unit, not something to reproduce here.)
fn config_path() -> PathBuf {
    base_dirs().config_home.join("idiolect").join("config.toml")
}

fn base_dirs() -> XdgBaseDirs {
    XdgBaseDirs::for_platform(Platform::host())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_file(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, body).expect("write config");
        path
    }

    #[test]
    fn it_reads_the_users_configured_command() {
        let dir = tempfile::tempdir().expect("temporary config directory");
        let path = config_file(
            &dir,
            "[daemon]\nlog_level = \"info\"\nnotify_command = \"/opt/custom-notifier\"\n",
        );

        assert_eq!(notify_command_from(&path), "/opt/custom-notifier");
    }

    #[test]
    fn an_explicitly_empty_command_is_honoured_as_notifications_off() {
        let dir = tempfile::tempdir().expect("temporary config directory");
        let path = config_file(&dir, "[daemon]\nnotify_command = \"\"\n");

        assert_eq!(
            notify_command_from(&path),
            "",
            "a user who turned notifications off must not be notified"
        );
    }

    #[test]
    fn a_missing_or_malformed_config_falls_back_to_the_default() {
        let dir = tempfile::tempdir().expect("temporary config directory");

        assert_eq!(
            notify_command_from(&dir.path().join("absent.toml")),
            "notify-send"
        );

        let broken = config_file(&dir, "this is not toml {{{");
        assert_eq!(notify_command_from(&broken), "notify-send");
    }

    #[test]
    fn a_config_without_a_daemon_section_uses_the_default() {
        let dir = tempfile::tempdir().expect("temporary config directory");
        let path = config_file(&dir, "[user]\ndefault_user_id = \"default\"\n");

        assert_eq!(notify_command_from(&path), "notify-send");
    }

    #[test]
    fn the_config_path_is_the_one_the_rest_of_the_workspace_resolves() {
        // Hand-rolling this resolution is how the engine ends up reading a
        // different file from everything else — an empty `XDG_CONFIG_HOME`
        // yields a CWD-relative path if you just join onto the raw value.
        let expected = XdgBaseDirs::for_platform(Platform::host())
            .config_home
            .join("idiolect")
            .join("config.toml");

        assert_eq!(config_path(), expected);
    }

    #[test]
    fn the_diagnostics_log_sits_under_the_data_home_not_the_cache() {
        // The notification tells the user to grep this file, so it must not be
        // somewhere a cleaner is entitled to delete.
        let log = diagnostics_log_path();
        let base = XdgBaseDirs::for_platform(Platform::host());

        assert!(log.starts_with(&base.data_home), "{}", log.display());
        assert!(!log.starts_with(&base.cache_home), "{}", log.display());
        assert_eq!(
            log.file_name().and_then(|name| name.to_str()),
            Some("engine.log")
        );
    }
}
