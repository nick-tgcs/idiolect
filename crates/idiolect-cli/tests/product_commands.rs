use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn product_command_groups_exist_with_safe_json_stubs() {
    let cases: &[(&[&str], &str)] = &[
        (&["service", "status", "--json"], "service status"),
        (&["service", "restart", "--json"], "service restart"),
        (&["models", "list", "--json"], "models list"),
        (
            &["models", "install", "whisper-medium-en", "--json"],
            "models install",
        ),
        (&["sessions", "list", "--json"], "sessions list"),
        (
            &["sessions", "show", "session-1", "--json"],
            "sessions show",
        ),
        (
            &["sessions", "delete", "session-1", "--json"],
            "sessions delete",
        ),
        (&["memory", "list", "--json"], "memory list"),
        (&["memory", "delete", "memory-1", "--json"], "memory delete"),
        (&["candidates", "list", "--json"], "candidates list"),
        (
            &["train", "export-manifest", "--json"],
            "train export-manifest",
        ),
        (&["train", "classify", "--json"], "train classify"),
        (&["train", "run", "--json"], "train run"),
        (&["adapters", "list", "--json"], "adapters list"),
        (
            &["adapters", "promote", "adapter-1", "--json"],
            "adapters promote",
        ),
        (&["adapters", "rollback", "--json"], "adapters rollback"),
        (
            &[
                "privacy",
                "delete-all",
                "--user",
                "default",
                "--confirm-delete",
                "--json",
            ],
            "privacy delete-all",
        ),
    ];

    for (args, command_name) in cases {
        let output = run_cli(args);
        assert!(
            !output.status.success(),
            "{command_name} must not fake success"
        );
        let json = stdout_json(output);
        assert_eq!(json["code"], "not-implemented", "{command_name}");
        assert_eq!(json["command"], *command_name, "{command_name}");
    }
}

#[test]
fn privacy_export_remains_real_command() {
    let db = unique_temp_db_path("product-privacy-export");
    let output = run_cli(&[
        "privacy",
        "export",
        "--user",
        "default",
        "--db",
        db.to_str().expect("temp db should be utf8"),
    ]);

    assert!(output.status.success());
    let json = stdout_json(output);
    assert_eq!(json["user"], "default");
    cleanup_db_path(db);
}

fn run_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args(args)
        .output()
        .expect("idiolect-cli command should run")
}

fn stdout_json(output: Output) -> Value {
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("stdout should be JSON: {error}: {stdout}"))
}

fn unique_temp_db_path(tag: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    env::temp_dir().join(format!(
        "idiolect-cli-{tag}-{}-{}.db",
        std::process::id(),
        now.as_nanos()
    ))
}

fn cleanup_db_path(path: PathBuf) {
    let _ = fs::remove_file(path);
}
