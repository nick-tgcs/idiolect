//! Daemon IPC client. Reuses the `idiolect-ipc` wire protocol (newline JSON,
//! handshake v1) rather than re-implementing it. Split into a [`DaemonSender`]
//! (the session's [`DaemonClient`]) and a [`DaemonReader`] (driven by the
//! engine's read thread / integration tests), both over clones of one socket.
//!
//! The daemon owns the microphone and is the single authority for recording
//! state, so the client advertises the `recording_status` feature and the engine
//! mirrors the pushes it receives. The sender holds the live socket behind a shared
//! handle so the read loop can swap it on reconnect (see [`reconnect`]) without
//! rebuilding the session that owns the sender.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{
    FEATURE_COMMIT, FEATURE_PREEDIT, FEATURE_RECORDING_STATUS, PROTOCOL_VERSION,
};
use idiolect_ipc::{ClientHello, CommitPreedit, IpcMessage, ReportCorrection};

use crate::session::DaemonClient;

/// The daemon socket, matching the daemon's default
/// (`$XDG_RUNTIME_DIR/idiolect.sock`, else `$HOME/.local/run/idiolect/...`).
#[must_use]
pub fn default_socket_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("idiolect.sock");
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("run")
            .join("idiolect")
            .join("idiolect.sock");
    }
    PathBuf::from("/tmp/idiolect.sock")
}

/// The write half of the daemon connection. Holds the live socket behind a shared
/// handle so a reconnect can swap it in one place — the session keeps its
/// `DaemonSender` and starts using the new socket transparently. `None` while
/// disconnected: sends are dropped until the read loop reconnects and refills it.
#[derive(Clone)]
pub struct DaemonSender {
    stream: Arc<Mutex<Option<UnixStream>>>,
}

pub struct DaemonReader {
    reader: BufReader<UnixStream>,
}

fn invalid_data<E: std::fmt::Display>(error: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn client_hello() -> IpcMessage {
    IpcMessage::ClientHello(ClientHello {
        client_name: "idiolect-ibus".to_owned(),
        protocol_version: PROTOCOL_VERSION,
        features: vec![
            FEATURE_PREEDIT.to_owned(),
            FEATURE_COMMIT.to_owned(),
            // The daemon is authoritative for recording state; ask for its pushes.
            FEATURE_RECORDING_STATUS.to_owned(),
        ],
    })
}

/// Send the handshake on `sender` and await the daemon's `ServerHello` on `reader`.
fn handshake(sender: &mut DaemonSender, reader: &mut DaemonReader) -> io::Result<()> {
    sender.send(&client_hello())?;
    match reader.read_message()? {
        IpcMessage::ServerHello(_) => Ok(()),
        other => Err(invalid_data(format!("expected ServerHello, got {other:?}"))),
    }
}

/// Connect to the daemon and complete the handshake, returning a sender/reader
/// pair over clones of the same connection.
pub fn connect(socket_path: &Path) -> io::Result<(DaemonSender, DaemonReader)> {
    let stream = UnixStream::connect(socket_path)?;
    let reader_stream = stream.try_clone()?;
    let mut sender = DaemonSender {
        stream: Arc::new(Mutex::new(Some(stream))),
    };
    let mut reader = DaemonReader {
        reader: BufReader::new(reader_stream),
    };
    handshake(&mut sender, &mut reader)?;
    Ok((sender, reader))
}

/// Re-establish a dropped connection, swapping the live socket inside the existing
/// `sender` (so the session that owns it keeps working) and returning a fresh
/// reader. The caller resets its session to idle afterwards; the daemon re-pushes
/// its authoritative `RecordingStatus` to resync.
pub fn reconnect(socket_path: &Path, sender: &DaemonSender) -> io::Result<DaemonReader> {
    let stream = UnixStream::connect(socket_path)?;
    let reader_stream = stream.try_clone()?;
    sender.replace_stream(Some(stream));
    let mut reader = DaemonReader {
        reader: BufReader::new(reader_stream),
    };
    let mut sender = sender.clone();
    handshake(&mut sender, &mut reader)?;
    Ok(reader)
}

impl DaemonSender {
    /// Send a raw message (used by tests and the [`DaemonClient`] impl). Returns
    /// `NotConnected` while the socket is being re-established.
    pub fn send(&mut self, message: &IpcMessage) -> io::Result<()> {
        let line = encode_json_line(message).map_err(invalid_data)?;
        let mut guard = self.stream.lock().expect("daemon stream mutex poisoned");
        match guard.as_mut() {
            Some(stream) => {
                stream.write_all(line.as_bytes())?;
                stream.flush()
            }
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "daemon connection is being re-established",
            )),
        }
    }

    /// Swap the live socket (on reconnect) or clear it (on disconnect).
    pub fn replace_stream(&self, stream: Option<UnixStream>) {
        *self.stream.lock().expect("daemon stream mutex poisoned") = stream;
    }

    /// Test-only: wrap an already-connected stream directly (no handshake), so
    /// reader-loop logic can be exercised over a bare `UnixStream::pair()`.
    #[cfg(all(test, feature = "ibus-engine"))]
    pub(crate) fn from_stream(stream: UnixStream) -> Self {
        Self {
            stream: Arc::new(Mutex::new(Some(stream))),
        }
    }
}

impl DaemonClient for DaemonSender {
    fn toggle(&mut self) {
        // Fire-and-forget: a failed send means the socket died; the read loop
        // notices the same break and reconnects, after which the daemon re-pushes
        // the authoritative state. Losing this one intent is harmless — the user
        // presses again — so we do not surface the error here.
        let _ = self.send(&IpcMessage::ToggleRecording);
    }
    fn commit(&mut self, final_text: &str) {
        let _ = self.send(&IpcMessage::CommitPreedit(CommitPreedit {
            text: final_text.to_owned(),
        }));
    }
    fn report_correction(&mut self, corrected_text: &str) {
        #[cfg(feature = "trace")]
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/idiolect-edit.log")
        {
            use std::io::Write;
            let _ = writeln!(f, "SEND ReportCorrection: {corrected_text:?}");
        }
        let _ = self.send(&IpcMessage::ReportCorrection(ReportCorrection {
            corrected_text: corrected_text.to_owned(),
        }));
    }
    fn cancel(&mut self) {
        let _ = self.send(&IpcMessage::CancelPreedit);
    }
}

impl DaemonReader {
    /// Block until one complete message arrives. Errors with `UnexpectedEof`
    /// when the daemon closes the connection (the engine reconnects).
    pub fn read_message(&mut self) -> io::Result<IpcMessage> {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "daemon closed the connection",
            ));
        }
        decode_json_line(&line).map_err(invalid_data)
    }
}
