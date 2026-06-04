use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use idiolect_adapter_opus::{OpusCodec, OpusCodecError};
use idiolect_adapter_sqlite::{
    FileAudioStore, FileAudioStoreError, SqliteMetadataStore, SqliteStorageError,
};
use idiolect_common::ids::ImeSessionId;
use idiolect_ipc::framing::{decode_json_line, encode_json_line, FramingError};
use idiolect_ipc::handshake::{negotiate_protocol, HandshakeError};
use idiolect_ipc::messages::{CommitPreedit, ErrorMessage, IpcMessage, PreeditUpdate};
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioStorePort, MetadataStorePort};

use crate::adapters::{RuntimeAdapterError, RuntimeAdapterProfile};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunLoopConfig {
    pub(crate) socket_path: PathBuf,
    pub(crate) database_path: PathBuf,
    pub(crate) audio_root: PathBuf,
    pub(crate) decoded_cache_root: PathBuf,
    pub(crate) user_id: String,
    pub(crate) shutdown_after_client: bool,
    pub(crate) adapter_profile: RuntimeAdapterProfile,
}

#[derive(Debug)]
pub(crate) struct RunLoopError {
    message: String,
    source: Option<Box<dyn Error + 'static>>,
}

impl RunLoopError {
    fn io(action: &str, error: std::io::Error) -> Self {
        Self {
            message: format!("io {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn framing(error: FramingError) -> Self {
        Self {
            message: format!("ipc framing failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn handshake(error: HandshakeError) -> Self {
        Self {
            message: format!("ipc handshake failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn storage(action: &str, error: SqliteStorageError) -> Self {
        Self {
            message: format!("storage {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn audio_store(action: &str, error: FileAudioStoreError) -> Self {
        Self {
            message: format!("audio store {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn codec(action: &str, error: OpusCodecError) -> Self {
        Self {
            message: format!("audio codec {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn serialization(error: serde_json::Error) -> Self {
        Self {
            message: format!("session id serialization failed: {error}"),
            source: Some(Box::new(error)),
        }
    }
}

impl Display for RunLoopError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RunLoopError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref()
    }
}

pub(crate) fn run(config: RunLoopConfig) -> Result<(), RunLoopError> {
    let listener = UnixListener::bind(&config.socket_path)
        .map_err(|error| RunLoopError::io("bind socket", error))?;
    let _cleanup = SocketCleanup {
        path: config.socket_path.clone(),
    };

    loop {
        let (stream, _) = listener
            .accept()
            .map_err(|error| RunLoopError::io("accept client", error))?;
        handle_connection(stream, &config)?;
        if config.shutdown_after_client {
            return Ok(());
        }
    }
}

fn handle_connection(mut stream: UnixStream, config: &RunLoopConfig) -> Result<(), RunLoopError> {
    let reader_stream = stream
        .try_clone()
        .map_err(|error| RunLoopError::io("clone unix stream", error))?;
    let mut reader = BufReader::new(reader_stream);
    let mut store = SqliteMetadataStore::open_path(&config.database_path)
        .map_err(|error| RunLoopError::storage("open", error))?;
    store
        .migrate()
        .map_err(|error| RunLoopError::storage("migrate", error))?;
    let audio_store =
        FileAudioStore::new(config.audio_root.clone(), config.decoded_cache_root.clone());
    let codec = OpusCodec::new();
    let mut active_session = None;
    let mut line = String::new();

    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| RunLoopError::io("read ipc line", error))?;
        if read == 0 {
            cancel_uncommitted_active_session(
                &mut store,
                &mut active_session,
                "daemon-disconnect",
            )?;
            return Ok(());
        }

        match decode_json_line(&line).map_err(RunLoopError::framing)? {
            IpcMessage::ClientHello(client) => {
                let response = negotiate_protocol(&client).map_err(RunLoopError::handshake)?;
                send_ipc_message(&mut stream, &IpcMessage::ServerHello(response))?;
            }
            IpcMessage::StartRecording => {
                cancel_uncommitted_active_session(&mut store, &mut active_session, "daemon-retry")?;
                match start_fixture_session(&mut store, &audio_store, &codec, config)? {
                    StartSessionOutcome::Started(started_session) => {
                        let text = started_session.current_text.clone();
                        active_session = Some(started_session);
                        send_ipc_message(
                            &mut stream,
                            &IpcMessage::PreeditUpdate(PreeditUpdate { text }),
                        )?;
                    }
                    StartSessionOutcome::Recoverable(error) => {
                        send_ipc_message(
                            &mut stream,
                            &IpcMessage::Error(ErrorMessage {
                                code: error.code().to_owned(),
                                message: error.to_string(),
                            }),
                        )?;
                    }
                }
            }
            IpcMessage::CommitPreedit(commit) => {
                commit_active_session(&mut store, &mut active_session, commit)?;
            }
            IpcMessage::CancelPreedit => {
                cancel_uncommitted_active_session(
                    &mut store,
                    &mut active_session,
                    "daemon-cancel",
                )?;
                active_session = None;
            }
            IpcMessage::ServerHello(_) | IpcMessage::PreeditUpdate(_) | IpcMessage::Error(_) => {
                send_ipc_message(
                    &mut stream,
                    &IpcMessage::Error(ErrorMessage {
                        code: "unexpected-message".to_owned(),
                        message: "message is not valid from client".to_owned(),
                    }),
                )?;
            }
        }
    }
}

#[derive(Clone, Debug)]
struct ActiveSession {
    session_id: ImeSessionId,
    current_text: String,
    finalized: bool,
}

enum StartSessionOutcome {
    Started(ActiveSession),
    Recoverable(RuntimeAdapterError),
}

fn start_fixture_session(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    codec: &OpusCodec,
    config: &RunLoopConfig,
) -> Result<StartSessionOutcome, RunLoopError> {
    let segment = match crate::adapters::capture_audio(&config.adapter_profile) {
        Ok(segment) => segment,
        Err(error) => return Ok(StartSessionOutcome::Recoverable(error)),
    };
    let encoded = codec
        .encode(&segment)
        .map_err(|error| RunLoopError::codec("encode fixture", error))?;
    let decoded = codec
        .decode(&encoded)
        .map_err(|error| RunLoopError::codec("decode fixture", error))?;
    let draft = match crate::adapters::transcribe_audio(&config.adapter_profile, &decoded) {
        Ok(draft) => draft,
        Err(error) => return Ok(StartSessionOutcome::Recoverable(error)),
    };
    let session_id = store
        .create_session(Some(&draft.text))
        .map_err(|error| RunLoopError::storage("create session", error))?;
    let utterance_id = utterance_id_for_session(session_id)?;
    audio_store
        .write_source_audio(&config.user_id, &utterance_id, &encoded)
        .map_err(|error| RunLoopError::audio_store("write source audio", error))?;

    Ok(StartSessionOutcome::Started(ActiveSession {
        session_id,
        current_text: draft.text,
        finalized: false,
    }))
}

fn commit_active_session(
    store: &mut SqliteMetadataStore,
    active_session: &mut Option<ActiveSession>,
    commit: CommitPreedit,
) -> Result<(), RunLoopError> {
    let Some(active) = active_session.as_mut() else {
        return Ok(());
    };

    if commit.text != active.current_text {
        store
            .record_preedit_change(active.session_id, &active.current_text, &commit.text, 0)
            .map_err(|error| RunLoopError::storage("record preedit", error))?;
        active.current_text = commit.text.clone();
    }

    let idempotency_key = idempotency_key("daemon-commit", active.session_id)?;
    store
        .commit_session(active.session_id, &commit.text, &idempotency_key)
        .map_err(|error| RunLoopError::storage("commit session", error))?;
    active.finalized = true;
    Ok(())
}

fn cancel_uncommitted_active_session(
    store: &mut SqliteMetadataStore,
    active_session: &mut Option<ActiveSession>,
    reason: &str,
) -> Result<(), RunLoopError> {
    let Some(active) = active_session.as_mut() else {
        return Ok(());
    };
    if active.finalized {
        return Ok(());
    }

    let idempotency_key = idempotency_key(reason, active.session_id)?;
    store
        .cancel_session(active.session_id, &idempotency_key)
        .map_err(|error| RunLoopError::storage("cancel session", error))?;
    active.finalized = true;
    Ok(())
}

fn utterance_id_for_session(session_id: ImeSessionId) -> Result<String, RunLoopError> {
    Ok(format!(
        "utterance:{}",
        serde_json::to_string(&session_id)
            .map_err(RunLoopError::serialization)?
            .trim_matches('"')
    ))
}

fn idempotency_key(prefix: &str, session_id: ImeSessionId) -> Result<String, RunLoopError> {
    Ok(format!(
        "{prefix}:{}",
        serde_json::to_string(&session_id).map_err(RunLoopError::serialization)?
    ))
}

fn send_ipc_message(stream: &mut UnixStream, message: &IpcMessage) -> Result<(), RunLoopError> {
    let line = encode_json_line(message).map_err(RunLoopError::framing)?;
    stream
        .write_all(line.as_bytes())
        .map_err(|error| RunLoopError::io("write ipc line", error))?;
    stream
        .flush()
        .map_err(|error| RunLoopError::io("flush ipc line", error))
}

struct SocketCleanup {
    path: PathBuf,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
