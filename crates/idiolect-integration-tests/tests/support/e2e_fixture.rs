use std::thread;

use idiolectd::runtime::{serve_fixture, FixtureServerConfig};

use crate::e2e::E2ePaths;

pub(crate) fn spawn_fixture_server(paths: &E2ePaths, transcript: &str) -> thread::JoinHandle<()> {
    let config = FixtureServerConfig {
        socket_path: paths.socket_path.clone(),
        db_path: paths.db_path.clone(),
        transcript: transcript.to_owned(),
    };

    thread::spawn(move || {
        serve_fixture(config).expect("fixture server should run");
    })
}
