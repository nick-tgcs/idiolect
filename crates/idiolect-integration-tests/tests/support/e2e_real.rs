use std::path::PathBuf;
use std::thread;

use crate::e2e::E2ePaths;

pub(crate) fn spawn_real_fixture_server(paths: &E2ePaths) -> thread::JoinHandle<()> {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = idiolectd::runtime::RealFixtureServerConfig {
        socket_path: paths.socket_path.clone(),
        db_path: paths.db_path.clone(),
        audio_fixture_path: repo_root.join("tests/fixtures/audio/restart_traffic_16khz_mono.wav"),
        whisper_model_path: repo_root.join("tests/fixtures/whisper/ggml-tiny.en.bin"),
    };

    thread::spawn(move || {
        idiolectd::runtime::serve_real_fixture(config).expect("real fixture server should run");
    })
}
