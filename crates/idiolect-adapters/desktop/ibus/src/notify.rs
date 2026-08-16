//! Where the engine gets the user's notification command.
//!
//! The engine is launched by IBus with no arguments, so unlike the daemon —
//! which systemd starts with an explicit `--config` — it has to resolve the
//! config file itself. It reads the same path the packaged unit uses, so one
//! setting governs both processes.

use std::path::{Path, PathBuf};

use idiolect_common::config::{DaemonConfig, IdiolectConfig};

/// The user's configured notify command.
#[must_use]
pub fn configured_notify_command() -> String {
    config_path().map_or_else(
        || DaemonConfig::default().notify_command,
        |path| notify_command_from(&path),
    )
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

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("idiolect").join("config.toml"))
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
}
