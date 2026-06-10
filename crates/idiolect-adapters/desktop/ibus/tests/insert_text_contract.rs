//! Integration test: the idiolect-ibus daemon IPC *client* (`ipc::connect` +
//! `DaemonReader`) correctly receives a server-sent `InsertText` over a REAL
//! unix socket, completing the handshake exactly as against the real daemon.
//!
//! This is the engine's half of the history "Insert" contract: the daemon emits
//! `InsertText` (covered by the daemon's own unit test); here we prove the engine
//! decodes it off the wire. No IBus / display needed — the engine then turns this
//! into a `CommitText` D-Bus signal, which the gated e2e asserts.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_ibus::ipc;
use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{InsertText, ServerHello, PROTOCOL_VERSION};
use idiolect_ipc::IpcMessage;

#[test]
fn engine_client_receives_insert_text_from_the_server() {
    let socket_path = temp_socket_path();
    let listener = UnixListener::bind(&socket_path).expect("bind socket");

    // Minimal server: complete the handshake, then push one InsertText, exactly
    // as the daemon does when the user clicks tray "Insert".
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut writer = stream.try_clone().expect("clone");
        let mut reader = BufReader::new(stream);

        let mut hello = String::new();
        reader.read_line(&mut hello).expect("read ClientHello");
        match decode_json_line(&hello).expect("decode ClientHello") {
            IpcMessage::ClientHello(_) => {}
            other => panic!("expected ClientHello, got {other:?}"),
        }

        let server_hello = IpcMessage::ServerHello(ServerHello {
            protocol_version: PROTOCOL_VERSION,
            accepted_features: vec![],
        });
        writer
            .write_all(encode_json_line(&server_hello).expect("encode").as_bytes())
            .expect("send ServerHello");

        let insert = IpcMessage::InsertText(InsertText {
            text: "Deploy traefik and nginx".to_owned(),
        });
        writer
            .write_all(encode_json_line(&insert).expect("encode").as_bytes())
            .expect("send InsertText");
        writer.flush().expect("flush");
    });

    let (_sender, mut reader) = ipc::connect(&socket_path).expect("client connects + handshakes");
    match reader.read_message().expect("read message") {
        IpcMessage::InsertText(insert) => assert_eq!(insert.text, "Deploy traefik and nginx"),
        other => panic!("expected InsertText, got {other:?}"),
    }

    server.join().expect("server thread");
    let _ = std::fs::remove_file(&socket_path);
}

fn temp_socket_path() -> PathBuf {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
    std::env::temp_dir().join(format!(
        "idiolect-insert-text-{}-{}.sock",
        std::process::id(),
        now.as_nanos()
    ))
}
