//! Integration test for the engine's daemon-reconnect path — the fix for the
//! field bug where a daemon restart left the IBus engine permanently deaf (its
//! one-shot socket died and every send silently failed forever).
//!
//! Drives the real `ipc::connect` / `ipc::reconnect` against a minimal server over
//! a real unix socket: after the first connection drops, the client reconnects,
//! the *same* `DaemonSender` keeps working (its socket was swapped underneath),
//! and the daemon's authoritative `RecordingStatus` push resyncs the client. No
//! IBus / display needed.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_ibus::ipc;
use idiolect_ibus::session::DaemonClient;
use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{RecordingStatus, ServerHello, PROTOCOL_VERSION};
use idiolect_ipc::IpcMessage;

#[test]
fn engine_reconnects_after_daemon_drop_and_resyncs_via_status_push() {
    let socket_path = temp_socket_path();
    let listener = UnixListener::bind(&socket_path).expect("bind socket");

    // Minimal daemon: serve TWO sequential connections on one socket. The first is
    // dropped right after the handshake (simulating a daemon restart); the second
    // handshakes, pushes an authoritative RecordingStatus, and echoes back the
    // toggle the reconnected client sends.
    let server = thread::spawn(move || {
        // Connection 1 — handshake, then drop (closes the socket on the client).
        let (stream, _) = listener.accept().expect("accept first connection");
        read_client_hello(&stream);
        write_message(
            &stream,
            &IpcMessage::ServerHello(ServerHello {
                protocol_version: PROTOCOL_VERSION,
                accepted_features: vec!["recording_status".to_owned()],
            }),
        );
        drop(stream);

        // Connection 2 — the reconnect. Handshake, push recording=true, then read
        // whatever the client sends over the rebuilt sender.
        let (stream, _) = listener.accept().expect("accept reconnect");
        read_client_hello(&stream);
        write_message(
            &stream,
            &IpcMessage::ServerHello(ServerHello {
                protocol_version: PROTOCOL_VERSION,
                accepted_features: vec!["recording_status".to_owned()],
            }),
        );
        write_message(
            &stream,
            &IpcMessage::RecordingStatus(RecordingStatus { recording: true }),
        );
        read_message(&stream)
    });

    let (mut sender, mut reader, reconcile) =
        ipc::connect(&socket_path).expect("first connect + handshake");
    assert!(
        !reconcile,
        "server advertised no reconcile, so it must not be negotiated"
    );

    // The server dropped connection 1 → the read loop sees the connection die.
    assert!(
        reader.read_message().is_err(),
        "reader should observe the dropped connection"
    );

    // Reconnect: the same sender's socket is swapped underneath it.
    let (mut reader, reconcile) =
        ipc::reconnect(&socket_path, &sender).expect("reconnect + handshake");
    assert!(
        !reconcile,
        "reconnect re-reads the (still absent) reconcile"
    );

    // The daemon resyncs us with its authoritative state.
    match reader.read_message().expect("status push after reconnect") {
        IpcMessage::RecordingStatus(RecordingStatus { recording }) => {
            assert!(recording, "daemon pushed recording=true");
        }
        other => panic!("expected RecordingStatus, got {other:?}"),
    }

    // The reconnected sender works again (the old bug: sends went nowhere forever).
    sender.toggle();

    let echoed = server.join().expect("server thread");
    assert_eq!(
        echoed,
        IpcMessage::ToggleRecording,
        "the rebuilt sender's toggle reached the new connection"
    );

    let _ = std::fs::remove_file(&socket_path);
}

fn read_client_hello(stream: &UnixStream) {
    match read_message(stream) {
        IpcMessage::ClientHello(_) => {}
        other => panic!("expected ClientHello, got {other:?}"),
    }
}

fn read_message(stream: &UnixStream) -> IpcMessage {
    let mut reader = BufReader::new(stream.try_clone().expect("clone for read"));
    let mut line = String::new();
    reader.read_line(&mut line).expect("read line");
    decode_json_line(&line).expect("decode message")
}

fn write_message(mut stream: &UnixStream, message: &IpcMessage) {
    let line = encode_json_line(message).expect("encode message");
    stream.write_all(line.as_bytes()).expect("write message");
    stream.flush().expect("flush message");
}

fn temp_socket_path() -> PathBuf {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
    std::env::temp_dir().join(format!(
        "idiolect-reconnect-{}-{}.sock",
        std::process::id(),
        now.as_nanos()
    ))
}
