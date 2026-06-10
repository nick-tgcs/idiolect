#[path = "support/e2e.rs"]
mod e2e;
#[path = "support/e2e_fixture.rs"]
mod e2e_fixture;

use std::io::BufReader;

use idiolect_ipc::messages::{CommitPreedit, IpcMessage, PreeditUpdate};

#[test]
fn fixture_full_stack_commit_records_preedit_commit_storage_and_candidate() {
    let paths = e2e::E2ePaths::new("commit");
    let server = e2e_fixture::spawn_fixture_server(&paths, "restart traffic");
    let mut stream = e2e::connect_client(&paths.socket_path);
    let mut reader = BufReader::new(stream.try_clone().expect("stream should clone"));

    e2e::send_hello(&mut stream, &mut reader);
    e2e::send_message(&mut stream, &IpcMessage::StartRecording);

    match e2e::read_message(&mut reader) {
        IpcMessage::PreeditUpdate(PreeditUpdate { text, .. }) => {
            assert_eq!(text, "restart traffic")
        }
        other => panic!("expected PreeditUpdate, got {other:?}"),
    }

    e2e::send_message(
        &mut stream,
        &IpcMessage::CommitPreedit(CommitPreedit {
            text: "restart Traefik".to_owned(),
        }),
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
    assert_eq!(store.event_count_for_test().expect("event count"), 3);

    paths.cleanup();
}

#[test]
fn fixture_full_stack_cancel_clears_preedit_and_records_no_candidate() {
    let paths = e2e::E2ePaths::new("cancel");
    let server = e2e_fixture::spawn_fixture_server(&paths, "open notes");
    let mut stream = e2e::connect_client(&paths.socket_path);
    let mut reader = BufReader::new(stream.try_clone().expect("stream should clone"));

    e2e::send_hello(&mut stream, &mut reader);
    e2e::send_message(&mut stream, &IpcMessage::StartRecording);

    match e2e::read_message(&mut reader) {
        IpcMessage::PreeditUpdate(PreeditUpdate { text, .. }) => assert_eq!(text, "open notes"),
        other => panic!("expected PreeditUpdate, got {other:?}"),
    }

    e2e::send_message(&mut stream, &IpcMessage::CancelPreedit);
    drop(reader);
    drop(stream);
    server.join().expect("server thread should finish");

    let store = e2e::open_store(&paths.db_path);
    assert_eq!(
        store
            .training_candidate_count_for_test()
            .expect("candidate count should query"),
        0
    );
    assert_eq!(store.event_count_for_test().expect("event count"), 2);

    paths.cleanup();
}

#[test]
fn fixture_full_stack_duplicate_commit_is_idempotent() {
    let paths = e2e::E2ePaths::new("duplicate-commit");
    let server = e2e_fixture::spawn_fixture_server(&paths, "restart traffic");
    let mut stream = e2e::connect_client(&paths.socket_path);
    let mut reader = BufReader::new(stream.try_clone().expect("stream should clone"));

    e2e::send_hello(&mut stream, &mut reader);
    e2e::send_message(&mut stream, &IpcMessage::StartRecording);
    let _preedit = e2e::read_message(&mut reader);

    let commit = IpcMessage::CommitPreedit(CommitPreedit {
        text: "restart Traefik".to_owned(),
    });
    e2e::send_message(&mut stream, &commit);
    e2e::send_message(&mut stream, &commit);
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
    assert_eq!(store.event_count_for_test().expect("event count"), 3);

    paths.cleanup();
}
