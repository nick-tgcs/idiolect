use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{ClientHello, IpcMessage, FEATURE_COMMIT, FEATURE_PREEDIT};

pub(crate) struct E2ePaths {
    pub db_path: PathBuf,
    pub socket_path: PathBuf,
}

impl E2ePaths {
    pub(crate) fn new(tag: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = env::temp_dir().join(format!(
            "idiolect-e2e-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("temp e2e root should be created");
        Self {
            db_path: root.join("idiolect.db"),
            socket_path: root.join("idiolect.sock"),
        }
    }

    pub(crate) fn cleanup(&self) {
        if let Some(root) = self.db_path.parent() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

pub(crate) fn connect_client(socket_path: &Path) -> UnixStream {
    let deadline_attempts = 500;
    for _ in 0..deadline_attempts {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return stream,
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
    panic!("client could not connect to {}", socket_path.display());
}

pub(crate) fn send_message(stream: &mut UnixStream, message: &IpcMessage) {
    let line = encode_json_line(message).expect("message should encode");
    stream
        .write_all(line.as_bytes())
        .expect("message should write");
    stream.flush().expect("message should flush");
}

pub(crate) fn read_message(reader: &mut BufReader<UnixStream>) -> IpcMessage {
    let mut line = String::new();
    let read = reader.read_line(&mut line).expect("message should read");
    assert!(read > 0, "server closed before sending a message");
    decode_json_line(&line).expect("message should decode")
}

pub(crate) fn send_hello(stream: &mut UnixStream, reader: &mut BufReader<UnixStream>) {
    send_message(
        stream,
        &IpcMessage::ClientHello(ClientHello {
            client_name: "idiolect-e2e-test".to_owned(),
            protocol_version: 1,
            features: vec![FEATURE_PREEDIT.to_owned(), FEATURE_COMMIT.to_owned()],
        }),
    );

    match read_message(reader) {
        IpcMessage::ServerHello(server) => {
            assert_eq!(server.protocol_version, 1);
            assert_eq!(
                server.accepted_features,
                vec![FEATURE_PREEDIT.to_owned(), FEATURE_COMMIT.to_owned()]
            );
        }
        other => panic!("expected ServerHello, got {other:?}"),
    }
}

pub(crate) fn open_store(path: &Path) -> SqliteMetadataStore {
    let mut store = SqliteMetadataStore::open_path(path).expect("database should open");
    store.migrate().expect("migrations should run");
    store
}
