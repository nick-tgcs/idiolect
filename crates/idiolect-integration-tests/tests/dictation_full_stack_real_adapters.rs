#[path = "support/e2e.rs"]
mod e2e;
#[path = "support/e2e_real.rs"]
mod e2e_real;

use std::io::BufReader;

use idiolect_ipc::messages::{CommitPreedit, IpcMessage, PreeditUpdate};

#[test]
fn real_media_full_stack_transcribes_fixture_and_commits_candidate() {
    let paths = e2e::E2ePaths::new("real-media");
    let server = e2e_real::spawn_real_fixture_server(&paths);
    let mut stream = e2e::connect_client(&paths.socket_path);
    let mut reader = BufReader::new(stream.try_clone().expect("stream should clone"));

    e2e::send_hello(&mut stream, &mut reader);
    e2e::send_message(&mut stream, &IpcMessage::StartRecording);

    let transcript = match e2e::read_message(&mut reader) {
        IpcMessage::PreeditUpdate(PreeditUpdate { text, .. }) => text,
        other => panic!("expected PreeditUpdate, got {other:?}"),
    };
    let lower_transcript = transcript.to_lowercase();
    assert!(lower_transcript.contains("restart"));
    assert!(lower_transcript.contains("traffic"));

    e2e::send_message(
        &mut stream,
        &IpcMessage::CommitPreedit(CommitPreedit { text: transcript }),
    );
    drop(reader);
    drop(stream);
    server.join().expect("server thread should finish");

    let store = e2e::open_store(&paths.db_path);
    assert_eq!(
        store
            .training_candidate_count_for_test()
            .expect("candidate count should query"),
        1
    );
    assert_eq!(store.event_count_for_test().expect("event count"), 2);

    paths.cleanup();
}
