use std::process::Command;

#[test]
fn doctor_command_reports_json_status() {
    let output = Command::new(env!("CARGO_BIN_EXE_idiolect-cli"))
        .args(["doctor", "--json"])
        .output()
        .expect("doctor command should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("\"storage\""));
    assert!(stdout.contains("\"ipc\""));
}
