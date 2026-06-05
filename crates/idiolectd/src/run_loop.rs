use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;

use idiolect_adapter_opus::{OpusCodec, OpusCodecError};
use idiolect_adapter_sqlite::{
    FileAudioStore, FileAudioStoreError, SqliteMetadataStore, SqliteStorageError,
};
use idiolect_adapters_desktop_clipboard::ArboardClipboard;
use idiolect_adapters_desktop_ksni::{KsniTray, TrayCallback};
use idiolect_application::use_cases::menu::{MenuUseCase, RecordingState};
use idiolect_common::config::HistoryConfig;
use idiolect_common::ids::ImeSessionId;
use idiolect_ipc::framing::{decode_json_line, encode_json_line, FramingError};
use idiolect_ipc::handshake::{negotiate_protocol, HandshakeError};
use idiolect_ipc::messages::{CommitPreedit, ErrorMessage, IpcMessage, PreeditUpdate};
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{HistoryEntry, MetadataStorePort, TrayIcon, TrayStatus};

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
    pub(crate) tray_callback_rx: Option<mpsc::Receiver<TrayCallback>>,
    pub(crate) history_config: HistoryConfig,
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

    // Initialize tray and clipboard for the daemon
    let (tray_callback_tx, tray_callback_rx) = mpsc::channel::<TrayCallback>();
    let mut tray = KsniTray::new(tray_callback_tx).map_err(|e| RunLoopError::storage("tray init", e))?;
    let mut clipboard = ArboardClipboard::new().map_err(|e| RunLoopError::storage("clipboard init", e))?;
    
    // Open database
    let mut store = SqliteMetadataStore::open_path(&config.database_path)
        .map_err(|error| RunLoopError::storage("open", error))?;
    store
        .migrate()
        .map_err(|error| RunLoopError::storage("migrate", error))?;
    
    // Prune history on startup
    let _ = store.prune_history(config.history_config.retention_days);
    
    // Set initial tray menu
    let history_entries = store.recent_history(config.history_config.max_entries).unwrap_or_default();
    let menu = MenuUseCase::new().get_menu(
        RecordingState::Idle,
        &history_entries,
        &config.history_config,
    );
    tray.set_menu(menu).map_err(|e| RunLoopError::storage("tray menu", e))?;
    tray.set_icon(TrayIcon::Idle).map_err(|e| RunLoopError::storage("tray icon", e))?;
    tray.set_tooltip("Idiolect — Ready").map_err(|e| RunLoopError::storage("tray tooltip", e))?;
    tray.set_status(TrayStatus::Passive).map_err(|e| RunLoopError::storage("tray status", e))?;
    
    // Spawn background pruning thread
    let prune_db_path = config.database_path.clone();
    let prune_retention_days = config.history_config.retention_days;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600)); // 1 hour
            if let Ok(mut store) = SqliteMetadataStore::open_path(&prune_db_path) {
                let _ = store.prune_history(prune_retention_days);
            }
        }
    });

    loop {
        let (stream, _) = listener
            .accept()
            .map_err(|error| RunLoopError::io("accept client", error))?;
        handle_connection(stream, &config, &mut tray, &mut clipboard, &mut store)?;
        if config.shutdown_after_client {
            return Ok(());
        }
    }
}

fn handle_connection(
    mut stream: UnixStream,
    config: &RunLoopConfig,
    tray: &mut KsniTray,
    clipboard: &mut ArboardClipboard,
    store: &mut SqliteMetadataStore,
) -> Result<(), RunLoopError> {
    let reader_stream = stream
        .try_clone()
        .map_err(|error| RunLoopError::io("clone unix stream", error))?;
    let mut reader = BufReader::new(reader_stream);
    let audio_store =
        FileAudioStore::new(config.audio_root.clone(), config.decoded_cache_root.clone());
    let codec = OpusCodec::new();
    let mut active_session = None;
    let mut line = String::new();

    loop {
        // Process tray callbacks
        if let Some(rx) = &config.tray_callback_rx {
            while let Ok(callback) = rx.try_recv() {
                handle_tray_callback(callback, tray, clipboard, store, &config.history_config)?;
            }
        }

        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| RunLoopError::io("read ipc line", error))?;
        if read == 0 {
            cancel_uncommitted_active_session(
                store,
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
                cancel_uncommitted_active_session(store, &mut active_session, "daemon-retry")?;
                match start_fixture_session(store, &audio_store, &codec, config)? {
                    StartSessionOutcome::Started(started_session) => {
                        let text = started_session.current_text.clone();
                        active_session = Some(started_session);
                        send_ipc_message(
                            &mut stream,
                            &IpcMessage::PreeditUpdate(PreeditUpdate { text }),
                        )?;
                        // Update tray to recording state
                        update_tray_for_recording(tray, store, &config.history_config, true)?;
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
                commit_active_session(store, &mut active_session, commit)?;
                // Update tray after commit
                update_tray_after_commit(tray, store, &config.history_config)?;
            }
            IpcMessage::CancelPreedit => {
                cancel_uncommitted_active_session(
                    store,
                    &mut active_session,
                    "daemon-cancel",
                )?;
                active_session = None;
                // Update tray after cancel
                update_tray_after_cancel(tray, store, &config.history_config)?;
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

fn handle_tray_callback(
    callback: TrayCallback,
    tray: &mut KsniTray,
    clipboard: &mut ArboardClipboard,
    store: &mut SqliteMetadataStore,
    history_config: &HistoryConfig,
) -> Result<(), RunLoopError> {
    let action = callback.0; // TrayCallback::Activate(String)
    
    if action.starts_with("insert:") {
        let id = action.strip_prefix("insert:").unwrap().parse::<i64>().unwrap();
        let entries = store.recent_history(100).map_err(|e| RunLoopError::storage("recent history", e))?;
        if let Some(entry) = entries.into_iter().find(|e| e.id == id) {
            // Note: We can't call commit_text here because we don't have InputMethodPort
            // The actual re-insertion would need to go through the IPC layer
            // For now, we just log it
            eprintln!("Insert requested for history entry {}: {}", id, entry.text);
        }
    } else if action.starts_with("copy:") {
        let id = action.strip_prefix("copy:").unwrap().parse::<i64>().unwrap();
        let entries = store.recent_history(100).map_err(|e| RunLoopError::storage("recent history", e))?;
        if let Some(entry) = entries.into_iter().find(|e| e.id == id) {
            clipboard.set_text(&entry.text).map_err(|e| RunLoopError::storage("clipboard", e))?;
        }
    } else if action.starts_with("delete:") {
        let id = action.strip_prefix("delete:").unwrap().parse::<i64>().unwrap();
        store.delete_history_entry(id).map_err(|e| RunLoopError::storage("delete history", e))?;
        // Refresh tray menu
        let history_entries = store.recent_history(history_config.max_entries).unwrap_or_default();
        let menu = MenuUseCase::new().get_menu(RecordingState::Idle, &history_entries, history_config);
        tray.set_menu(menu).map_err(|e| RunLoopError::storage("tray menu", e))?;
    } else if action.starts_with("settings:retention:") {
        let days = action.strip_prefix("settings:retention:").unwrap().parse::<u32>().unwrap();
        // Update config and persist
        // For now, just refresh menu
        let history_entries = store.recent_history(history_config.max_entries).unwrap_or_default();
        let menu = MenuUseCase::new().get_menu(RecordingState::Idle, &history_entries, history_config);
        tray.set_menu(menu).map_err(|e| RunLoopError::storage("tray menu", e))?;
    } else if action.starts_with("settings:max_entries:") {
        let max = action.strip_prefix("settings:max_entries:").unwrap().parse::<u32>().unwrap();
        // Update config and persist
        let history_entries = store.recent_history(history_config.max_entries).unwrap_or_default();
        let menu = MenuUseCase::new().get_menu(RecordingState::Idle, &history_entries, history_config);
        tray.set_menu(menu).map_err(|e| RunLoopError::storage("tray menu", e))?;
    } else if action == "start_recording" {
        // This would be handled by the IPC layer
        eprintln!("Start recording requested from tray");
    } else if action == "stop_recording" {
        eprintln!("Stop recording requested from tray");
    } else if action == "cancel" {
        eprintln!("Cancel requested from tray");
    }
    
    Ok(())
}

fn update_tray_for_recording(
    tray: &mut KsniTray,
    store: &mut SqliteMetadataStore,
    history_config: &HistoryConfig,
    recording: bool,
) -> Result<(), RunLoopError> {
    let history_entries = store.recent_history(history_config.max_entries).unwrap_or_default();
    let state = if recording { RecordingState::Recording } else { RecordingState::Idle };
    let menu = MenuUseCase::new().get_menu(state, &history_entries, history_config);
    tray.set_menu(menu).map_err(|e| RunLoopError::storage("tray menu", e))?;
    tray.set_icon(if recording { TrayIcon::Recording } else { TrayIcon::Idle })
        .map_err(|e| RunLoopError::storage("tray icon", e))?;
    tray.set_tooltip(if recording { "Idiolect — Recording…" } else { "Idiolect — Ready" })
        .map_err(|e| RunLoopError::storage("tray tooltip", e))?;
    Ok(())
}

fn update_tray_after_commit(
    tray: &mut KsniTray,
    store: &mut SqliteMetadataStore,
    history_config: &HistoryConfig,
) -> Result<(), RunLoopError> {
    update_tray_for_recording(tray, store, history_config, false)
}

fn update_tray_after_cancel(
    tray: &mut KsniTray,
    store: &mut SqliteMetadataStore,
    history_config: &HistoryConfig,
) -> Result<(), RunLoopError> {
    update_tray_for_recording(tray, store, history_config, false)
}

struct SocketCleanup {
    path: PathBuf,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
