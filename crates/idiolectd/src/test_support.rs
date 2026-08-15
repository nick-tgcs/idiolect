//! Shared support for subprocess-notification tests.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(crate) struct NotificationRecorder {
    _directory: tempfile::TempDir,
    command: String,
    log: PathBuf,
}

impl NotificationRecorder {
    pub(crate) fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary notifier directory");
        let log = directory.path().join("notifications.log");
        let notifier = directory.path().join("notify");
        std::fs::write(
            &notifier,
            format!(
                "#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"$2\" >> \"{}\"\n",
                log.display()
            ),
        )
        .expect("write notification recorder");
        std::fs::set_permissions(&notifier, std::fs::Permissions::from_mode(0o755))
            .expect("chmod notification recorder");

        Self {
            _directory: directory,
            command: notifier.to_string_lossy().into_owned(),
            log,
        }
    }

    pub(crate) fn command(&self) -> &str {
        &self.command
    }

    pub(crate) fn log_path(&self) -> &Path {
        &self.log
    }

    pub(crate) fn wait(&self) -> String {
        wait_for_nonempty_file(&self.log)
    }
}

pub(crate) fn wait_for_nonempty_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if !contents.is_empty() {
                return contents;
            }
        }
        assert!(Instant::now() < deadline, "notification was not emitted");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_ignores_an_empty_file_until_the_notifier_writes() {
        let dir = tempfile::tempdir().expect("temporary notifier directory");
        let log = dir.path().join("notifications.log");
        std::fs::write(&log, "").expect("create empty notification log");
        let writer_log = log.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            std::fs::write(writer_log, "ready").expect("write notification");
        });

        let contents = wait_for_nonempty_file(&log);
        writer.join().expect("notification writer");

        assert_eq!(contents, "ready");
    }

    #[test]
    fn recorder_removes_its_temporary_directory_on_drop() {
        let directory = {
            let recorder = NotificationRecorder::new();
            let directory = recorder._directory.path().to_owned();
            assert!(directory.exists());
            directory
        };

        assert!(!directory.exists());
    }
}
