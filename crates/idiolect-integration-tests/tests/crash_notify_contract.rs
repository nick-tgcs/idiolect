//! Contract for `idiolect-crash-notify`, the ExecStopPost hook that makes a
//! daemon crash VISIBLE to the user. The daemon cannot report its own death
//! (a segfault in native code never returns control), so systemd runs this
//! script after every stop with `$SERVICE_RESULT`/`$EXIT_STATUS` set, and it
//! must turn abnormal exits into a desktop notification — with a tailored
//! explanation when the journal shows the GPU-out-of-memory signature.
//!
//! The script's collaborators are injectable for tests: the notifier command,
//! the journal command, and the throttle stamp file.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/debian/usr/bin/idiolect-crash-notify")
        .canonicalize()
        .expect("crash-notify script must exist in packaging")
}

struct Stub {
    dir: PathBuf,
}

impl Stub {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "idiolect-crash-notify-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("stub dir");
        Self { dir }
    }

    /// A notify-send stand-in that appends "<summary>|<body>" lines.
    fn notify_recorder(&self) -> PathBuf {
        let log = self.notifications_log();
        let path = self.dir.join("notify-recorder.sh");
        fs::write(
            &path,
            format!("#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"$2\" >> \"{}\"\n", log.display()),
        )
        .expect("write notify recorder");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    fn notifications_log(&self) -> PathBuf {
        self.dir.join("notifications.log")
    }

    /// A journal stand-in printing the given tail.
    fn journal(&self, contents: &str) -> PathBuf {
        let text = self.dir.join("journal.txt");
        fs::write(&text, contents).expect("write journal fixture");
        let path = self.dir.join("journal.sh");
        fs::write(&path, format!("#!/bin/sh\ncat \"{}\"\n", text.display()))
            .expect("write journal stub");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    /// A nvidia-smi stand-in printing the given compute-apps CSV.
    fn gpu_apps(&self, contents: &str) -> PathBuf {
        let text = self.dir.join("gpu.txt");
        fs::write(&text, contents).expect("write gpu fixture");
        let path = self.dir.join("gpu.sh");
        fs::write(&path, format!("#!/bin/sh\ncat \"{}\"\n", text.display()))
            .expect("write gpu stub");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    fn run(&self, service_result: &str, exit_status: &str, journal: &Path) {
        self.run_with_gpu(service_result, exit_status, journal, &self.gpu_apps(""));
    }

    fn run_with_gpu(&self, service_result: &str, exit_status: &str, journal: &Path, gpu: &Path) {
        let status = Command::new(script_path())
            .env("SERVICE_RESULT", service_result)
            .env("EXIT_STATUS", exit_status)
            .env("IDIOLECT_NOTIFY_CMD", self.notify_recorder())
            .env("IDIOLECT_JOURNAL_CMD", journal)
            .env("IDIOLECT_GPU_CMD", gpu)
            .env("IDIOLECT_THROTTLE_FILE", self.dir.join("throttle.stamp"))
            .status()
            .expect("script should run");
        assert!(status.success(), "the hook must never fail the unit");
    }

    fn notifications(&self) -> String {
        fs::read_to_string(self.notifications_log()).unwrap_or_default()
    }
}

#[test]
fn a_clean_stop_is_silent() {
    // `systemctl restart`/`stop` also runs ExecStopPost — those must not toast.
    let stub = Stub::new("clean");
    let journal = stub.journal("ordinary shutdown lines");
    stub.run("success", "0", &journal);
    assert_eq!(stub.notifications(), "", "no notification on a clean stop");
}

#[test]
fn a_crash_notifies_with_the_failure_detail() {
    let stub = Stub::new("crash");
    let journal = stub.journal("some unrelated lines");
    stub.run("core-dump", "SEGV", &journal);
    let log = stub.notifications();
    assert_eq!(log.lines().count(), 1, "exactly one notification: {log:?}");
    assert!(log.contains("Idiolect"), "names the app: {log:?}");
    assert!(
        log.contains("core-dump") && log.contains("SEGV"),
        "carries systemd's failure detail: {log:?}"
    );
}

const OOM_JOURNAL: &str = "whisper_init_state: compute buffer (decode) = 99.12 MB\n\
    ggml_backend_cuda_buffer_type_alloc_buffer: allocating 336.00 MiB on device 0: \
    cudaMalloc failed: out of memory\n\
    whisper_kv_cache_init: failed to allocate memory for the kv cache\n";

#[test]
fn the_gpu_oom_signature_names_the_actual_gpu_hog() {
    // The crash we've seen: whisper segfaults when another app holds the GPU.
    // The notification must name whatever is ACTUALLY holding the memory at
    // crash time (queried, never hard-coded — it may be anything).
    let stub = Stub::new("oom");
    let journal = stub.journal(OOM_JOURNAL);
    let gpu = stub.gpu_apps(
        "2369167, /snap/telegram-desktop/7006/usr/bin/telegram-desktop, 48\n\
         3003268, /usr/local/lib/ollama/llama-server, 18158\n",
    );
    stub.run_with_gpu("core-dump", "SEGV", &journal, &gpu);
    let log = stub.notifications();
    assert_eq!(log.lines().count(), 1, "{log:?}");
    assert!(
        log.contains("GPU") && log.to_lowercase().contains("memory"),
        "explains the GPU cause: {log:?}"
    );
    assert!(
        log.contains("llama-server (18158 MiB)"),
        "names the biggest CURRENT holder, not a guess: {log:?}"
    );
    assert!(
        !log.contains("telegram"),
        "only the dominant consumer is named: {log:?}"
    );
}

#[test]
fn the_gpu_oom_message_degrades_gracefully_when_no_holder_is_visible() {
    // nvidia-smi unavailable or shows nothing (the holder may have exited):
    // still explain the cause, point at nvidia-smi, and accuse nobody.
    let stub = Stub::new("oom-unknown");
    let journal = stub.journal(OOM_JOURNAL);
    stub.run("core-dump", "SEGV", &journal);
    let log = stub.notifications();
    assert_eq!(log.lines().count(), 1, "{log:?}");
    assert!(
        log.contains("GPU") && log.contains("nvidia-smi"),
        "tells the user how to find the culprit themselves: {log:?}"
    );
}

#[test]
fn rapid_crash_loops_toast_once_not_per_restart() {
    // StartLimitBurst allows several restarts in a row; one toast is enough.
    let stub = Stub::new("throttle");
    let journal = stub.journal("lines");
    stub.run("core-dump", "SEGV", &journal);
    stub.run("core-dump", "SEGV", &journal);
    stub.run("core-dump", "SEGV", &journal);
    assert_eq!(
        stub.notifications().lines().count(),
        1,
        "back-to-back crashes within the throttle window notify once"
    );
}
