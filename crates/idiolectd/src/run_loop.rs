use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use idiolect_adapter_clipboard::{ArboardClipboard, ArboardClipboardError};
use idiolect_adapter_crypto::{
    ChaCha20Poly1305Cipher, CryptoError, EncryptionKeyPort, EncryptionPort, FileKey,
};
use idiolect_adapter_ksni::{KsniTray, KsniTrayError, TrayCallback};
use idiolect_adapter_opus::{OpusCodec, OpusCodecError};
use idiolect_adapter_sqlite::{
    FileAudioStore, FileAudioStoreError, SqliteMetadataStore, SqliteStorageError,
};
use idiolect_application::use_cases::history::ClipboardPort;
use idiolect_application::use_cases::maintenance::MaintenanceUseCase;
use idiolect_application::use_cases::menu::{
    MenuUseCase, RecordingState, MAX_ENTRY_CHOICES, RETENTION_DAY_CHOICES,
};
use idiolect_common::config::HistoryConfig;
use idiolect_common::ids::ImeSessionId;
use idiolect_ipc::framing::{decode_json_line, encode_json_line, FramingError};
use idiolect_ipc::handshake::{negotiate_protocol, HandshakeError};
use idiolect_ipc::messages::{
    CommitPreedit, ErrorMessage, HistoryCopyResponse, HistoryReinsertResponse, IpcMessage,
    PreeditUpdate,
};
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioStorePort, MetadataStorePort, TrayIcon, TrayPort, TrayStatus};
use tokio::sync::watch;

use crate::adapters::{RuntimeAdapterError, RuntimeAdapterProfile};

#[derive(Debug)]
pub(crate) struct RunLoopConfig {
    pub(crate) socket_path: PathBuf,
    pub(crate) database_path: PathBuf,
    pub(crate) audio_root: PathBuf,
    pub(crate) decoded_cache_root: PathBuf,
    pub(crate) user_id: String,
    pub(crate) shutdown_after_client: bool,
    pub(crate) adapter_profile: RuntimeAdapterProfile,
    /// History defaults from the config file; per-setting overrides persisted in
    /// the `tray_settings` table take precedence at runtime.
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

    pub(crate) fn storage(action: &str, error: SqliteStorageError) -> Self {
        Self {
            message: format!("storage {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    pub(crate) fn tray(action: &str, error: KsniTrayError) -> Self {
        Self {
            message: format!("tray {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    pub(crate) fn clipboard(action: &str, error: ArboardClipboardError) -> Self {
        Self {
            message: format!("clipboard {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    pub(crate) fn audio_store(action: &str, error: FileAudioStoreError) -> Self {
        Self {
            message: format!("audio store {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    pub(crate) fn codec(action: &str, error: OpusCodecError) -> Self {
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

    fn crypto(action: &str, error: CryptoError) -> Self {
        Self {
            message: format!("history encryption {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }
}

/// Builds the at-rest history cipher when `encrypt_at_rest` is enabled. The key
/// is stored next to the database with `0600` permissions.
fn build_history_cipher(
    database_path: &Path,
    config: &HistoryConfig,
) -> Result<Option<Box<dyn EncryptionPort + Send + Sync>>, RunLoopError> {
    if !config.encrypt_at_rest {
        return Ok(None);
    }
    let key_path = database_path
        .parent()
        .map_or_else(|| PathBuf::from("history.key"), |parent| parent.join("history.key"));
    let key = FileKey::new(key_path)
        .load_or_create_key()
        .map_err(|error| RunLoopError::crypto("key load", error))?;
    Ok(Some(Box::new(ChaCha20Poly1305Cipher::new(key))))
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

    // Open the metadata store and apply migrations.
    let mut store = SqliteMetadataStore::open_path(&config.database_path)
        .map_err(|error| RunLoopError::storage("open", error))?;
    store
        .migrate()
        .map_err(|error| RunLoopError::storage("migrate", error))?;

    // Enable at-rest history encryption when configured.
    let mut store = match build_history_cipher(&config.database_path, &config.history_config)? {
        Some(cipher) => store.with_history_cipher(cipher),
        None => store,
    };

    // Prune once on startup so an idle daemon still enforces retention.
    let effective = effective_history_config(&store, &config.history_config);
    if effective.retention_days > 0 {
        match store.prune_history(effective.retention_days) {
            Ok(removed) => eprintln!("startup prune removed {removed} history entries"),
            Err(error) => eprintln!("startup prune failed: {error}"),
        }
    }

    // A single tray owned here; its callbacks are drained inside the connection loop.
    let (tray_callback_tx, tray_callback_rx) = mpsc::channel::<TrayCallback>();
    let mut tray =
        KsniTray::new(tray_callback_tx).map_err(|error| RunLoopError::tray("tray init", error))?;
    let mut clipboard =
        ArboardClipboard::new().map_err(|error| RunLoopError::clipboard("clipboard init", error))?;

    refresh_tray_menu(&mut tray, &store, &config.history_config, RecordingState::Idle)?;
    tray.set_icon(TrayIcon::Idle)
        .map_err(|error| RunLoopError::tray("tray icon", error))?;
    tray.set_tooltip("Idiolect — Ready")
        .map_err(|error| RunLoopError::tray("tray tooltip", error))?;
    tray.set_status(TrayStatus::Passive)
        .map_err(|error| RunLoopError::tray("tray status", error))?;

    // Background pruning task on a dedicated thread with its own tokio runtime so
    // the loop is actually driven (a spawned task on an undriven runtime never runs).
    let (maintenance_shutdown_tx, maintenance_shutdown_rx) = watch::channel(());
    let maintenance_db = config.database_path.clone();
    let maintenance_defaults = config.history_config.clone();
    let maintenance_handle = thread::Builder::new()
        .name("idiolect-maintenance".to_owned())
        .spawn(move || {
            run_maintenance(maintenance_db, maintenance_defaults, maintenance_shutdown_rx);
        })
        .map_err(|error| RunLoopError::io("spawn maintenance thread", error))?;

    let result = (|| {
        loop {
            let (stream, _) = listener
                .accept()
                .map_err(|error| RunLoopError::io("accept client", error))?;
            handle_connection(stream, &config, &mut tray, &mut clipboard, &mut store, &tray_callback_rx)?;
            if config.shutdown_after_client {
                return Ok(());
            }
        }
    })();

    // Signal the maintenance task to stop and wait for it to unwind.
    let _ = maintenance_shutdown_tx.send(());
    let _ = maintenance_handle.join();
    result
}

/// Maintenance task entry point: opens its own store connection and drives the
/// pruning loop on a current-thread tokio runtime until shutdown.
fn run_maintenance(
    database_path: PathBuf,
    defaults: HistoryConfig,
    shutdown_rx: watch::Receiver<()>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("maintenance: failed to build runtime: {error}");
            return;
        }
    };

    runtime.block_on(async move {
        let store = match SqliteMetadataStore::open_path(&database_path) {
            Ok(store) => store,
            Err(error) => {
                eprintln!("maintenance: failed to open store: {error}");
                return;
            }
        };
        let config = effective_history_config(&store, &defaults);
        let maintenance = MaintenanceUseCase::new(store, config, shutdown_rx);
        if let Err(error) = maintenance.run_pruning_loop().await {
            eprintln!("maintenance: pruning loop error: {error}");
        }
    });
}

/// Resolves the active history configuration, layering persisted `tray_settings`
/// overrides on top of the config-file defaults. The `tray_settings` table is the
/// single source of truth at runtime.
fn effective_history_config(store: &SqliteMetadataStore, defaults: &HistoryConfig) -> HistoryConfig {
    let settings = store.get_all_tray_settings().unwrap_or_default();
    let retention_days = settings
        .get("retention_days")
        .and_then(|value| value.parse().ok())
        .unwrap_or(defaults.retention_days);
    let max_entries = settings
        .get("max_entries")
        .and_then(|value| value.parse().ok())
        .unwrap_or(defaults.max_entries);
    HistoryConfig {
        retention_days,
        max_entries,
        clipboard_auto_clear_secs: defaults.clipboard_auto_clear_secs,
        encrypt_at_rest: defaults.encrypt_at_rest,
    }
}

/// Rebuilds and installs the tray menu from current storage state.
fn refresh_tray_menu(
    tray: &mut KsniTray,
    store: &SqliteMetadataStore,
    defaults: &HistoryConfig,
    recording_state: RecordingState,
) -> Result<(), RunLoopError> {
    let config = effective_history_config(store, defaults);
    let entries = store
        .recent_history(config.max_entries)
        .map_err(|error| RunLoopError::storage("recent history", error))?;
    let menu = MenuUseCase::new().get_menu(recording_state, &entries, &config);
    tray.set_menu(menu)
        .map_err(|error| RunLoopError::tray("tray menu", error))
}

fn handle_connection(
    mut stream: UnixStream,
    config: &RunLoopConfig,
    tray: &mut KsniTray,
    clipboard: &mut ArboardClipboard,
    store: &mut SqliteMetadataStore,
    tray_callback_rx: &mpsc::Receiver<TrayCallback>,
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
        // Drain any pending tray callbacks before blocking on the next IPC line.
        while let Ok(callback) = tray_callback_rx.try_recv() {
            handle_tray_callback(callback, tray, clipboard, store, &config.history_config)?;
        }

        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| RunLoopError::io("read ipc line", error))?;
        if read == 0 {
            cancel_uncommitted_active_session(store, &mut active_session, "daemon-disconnect")?;
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
                        update_tray_recording_state(tray, store, &config.history_config, RecordingState::Recording)?;
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
                update_tray_recording_state(tray, store, &config.history_config, RecordingState::Idle)?;
            }
            IpcMessage::CancelPreedit => {
                cancel_uncommitted_active_session(store, &mut active_session, "daemon-cancel")?;
                active_session = None;
                update_tray_recording_state(tray, store, &config.history_config, RecordingState::Idle)?;
            }
            IpcMessage::HistoryReinsert(message) => {
                let response = reinsert_entry(
                    store,
                    clipboard,
                    message.id,
                    config.history_config.clipboard_auto_clear_secs,
                )?;
                send_ipc_message(&mut stream, &IpcMessage::HistoryReinsertResponse(response))?;
            }
            IpcMessage::HistoryCopy(message) => {
                let response = copy_entry(
                    store,
                    clipboard,
                    message.id,
                    config.history_config.clipboard_auto_clear_secs,
                )?;
                send_ipc_message(&mut stream, &IpcMessage::HistoryCopyResponse(response))?;
            }
            IpcMessage::HistoryReinsertResponse(_) | IpcMessage::HistoryCopyResponse(_) => {
                send_ipc_message(
                    &mut stream,
                    &IpcMessage::Error(ErrorMessage {
                        code: "unexpected-message".to_owned(),
                        message: "response message not expected from client".to_owned(),
                    }),
                )?;
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

/// Re-insert a history entry. Server-side IME injection is not yet available, so
/// the text is placed on the system clipboard for the user to paste. The response
/// reflects the real clipboard result — it never reports success without effect.
fn reinsert_entry(
    store: &SqliteMetadataStore,
    clipboard: &mut ArboardClipboard,
    id: i64,
    auto_clear_secs: u64,
) -> Result<HistoryReinsertResponse, RunLoopError> {
    let Some(entry) = store
        .get_history_entry(id)
        .map_err(|error| RunLoopError::storage("get history entry", error))?
    else {
        return Ok(HistoryReinsertResponse {
            success: false,
            error: Some(format!("history entry {id} not found")),
        });
    };

    Ok(match clipboard.set_text(&entry.text) {
        Ok(()) => {
            schedule_clipboard_clear(entry.text, auto_clear_secs);
            HistoryReinsertResponse {
                success: true,
                error: None,
            }
        }
        Err(error) => HistoryReinsertResponse {
            success: false,
            error: Some(format!("clipboard error: {error}")),
        },
    })
}

/// Copy a history entry's text to the system clipboard.
fn copy_entry(
    store: &SqliteMetadataStore,
    clipboard: &mut ArboardClipboard,
    id: i64,
    auto_clear_secs: u64,
) -> Result<HistoryCopyResponse, RunLoopError> {
    let Some(entry) = store
        .get_history_entry(id)
        .map_err(|error| RunLoopError::storage("get history entry", error))?
    else {
        return Ok(HistoryCopyResponse {
            success: false,
            error: Some(format!("history entry {id} not found")),
        });
    };

    Ok(match clipboard.set_text(&entry.text) {
        Ok(()) => {
            schedule_clipboard_clear(entry.text, auto_clear_secs);
            HistoryCopyResponse {
                success: true,
                error: None,
            }
        }
        Err(error) => HistoryCopyResponse {
            success: false,
            error: Some(format!("clipboard error: {error}")),
        },
    })
}

/// Schedules a best-effort clipboard clear after `secs` seconds. The clear only
/// happens if the clipboard still holds `text` (so a newer copy by the user is
/// never clobbered). `secs == 0` disables auto-clear. Uses a fresh clipboard
/// handle inside the worker thread since the daemon's handle is not `Send`.
fn schedule_clipboard_clear(text: String, secs: u64) {
    if secs == 0 {
        return;
    }
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(secs));
        let Ok(mut clipboard) = ArboardClipboard::new() else {
            return;
        };
        let current = clipboard.get_text().ok();
        if should_clear_clipboard(current.as_deref(), &text) {
            let _ = clipboard.set_text("");
        }
    });
}

/// Decides whether the clipboard should be auto-cleared: only when it still
/// holds exactly the text we previously placed there.
fn should_clear_clipboard(current: Option<&str>, expected: &str) -> bool {
    current == Some(expected)
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

/// Handles an activated tray menu item. Parsing is total — a malformed action id
/// is logged and ignored rather than panicking the daemon.
fn handle_tray_callback(
    callback: TrayCallback,
    tray: &mut KsniTray,
    clipboard: &mut ArboardClipboard,
    store: &mut SqliteMetadataStore,
    defaults: &HistoryConfig,
) -> Result<(), RunLoopError> {
    let TrayCallback::Activate(action) = callback;

    if let Some(id) = parse_id_suffix(&action, "insert:") {
        let _ = reinsert_entry(store, clipboard, id, defaults.clipboard_auto_clear_secs)?;
    } else if let Some(id) = parse_id_suffix(&action, "copy:") {
        let _ = copy_entry(store, clipboard, id, defaults.clipboard_auto_clear_secs)?;
    } else if let Some(id) = parse_id_suffix(&action, "delete:") {
        match store.delete_history_entry(id) {
            Ok(()) => refresh_tray_menu(tray, store, defaults, RecordingState::Idle)?,
            Err(error) => eprintln!("tray delete of entry {id} failed: {error}"),
        }
    } else if let Some(index) = parse_index_suffix(&action, "settings:retention:") {
        if let Some(days) = RETENTION_DAY_CHOICES.get(index) {
            store
                .set_tray_setting("retention_days", &days.to_string())
                .map_err(|error| RunLoopError::storage("set retention_days", error))?;
            refresh_tray_menu(tray, store, defaults, RecordingState::Idle)?;
        } else {
            eprintln!("tray retention index out of range: {index}");
        }
    } else if let Some(index) = parse_index_suffix(&action, "settings:max_entries:") {
        if let Some(max) = MAX_ENTRY_CHOICES.get(index) {
            store
                .set_tray_setting("max_entries", &max.to_string())
                .map_err(|error| RunLoopError::storage("set max_entries", error))?;
            refresh_tray_menu(tray, store, defaults, RecordingState::Idle)?;
        } else {
            eprintln!("tray max_entries index out of range: {index}");
        }
    } else {
        // start_recording / stop_recording / cancel originate from the IME client
        // over IPC, not the tray; ignore anything else.
        eprintln!("unhandled tray action: {action}");
    }

    Ok(())
}

fn parse_id_suffix(action: &str, prefix: &str) -> Option<i64> {
    action.strip_prefix(prefix).and_then(|rest| rest.parse().ok())
}

fn parse_index_suffix(action: &str, prefix: &str) -> Option<usize> {
    action.strip_prefix(prefix).and_then(|rest| rest.parse().ok())
}

fn update_tray_recording_state(
    tray: &mut KsniTray,
    store: &SqliteMetadataStore,
    defaults: &HistoryConfig,
    state: RecordingState,
) -> Result<(), RunLoopError> {
    refresh_tray_menu(tray, store, defaults, state)?;
    let recording = matches!(state, RecordingState::Recording);
    tray.set_icon(if recording {
        TrayIcon::Recording
    } else {
        TrayIcon::Idle
    })
    .map_err(|error| RunLoopError::tray("tray icon", error))?;
    tray.set_tooltip(if recording {
        "Idiolect — Recording…"
    } else {
        "Idiolect — Ready"
    })
    .map_err(|error| RunLoopError::tray("tray tooltip", error))?;
    Ok(())
}

struct SocketCleanup {
    path: PathBuf,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_id_suffix, parse_index_suffix, should_clear_clipboard};

    #[test]
    fn clipboard_clears_only_when_unchanged() {
        assert!(should_clear_clipboard(Some("hello"), "hello"));
        assert!(!should_clear_clipboard(Some("changed"), "hello"));
        assert!(!should_clear_clipboard(None, "hello"));
    }

    #[test]
    fn id_and_index_parsing_is_total() {
        assert_eq!(parse_id_suffix("delete:42", "delete:"), Some(42));
        assert_eq!(parse_id_suffix("delete:nan", "delete:"), None);
        assert_eq!(parse_id_suffix("copy:1", "delete:"), None);
        assert_eq!(parse_index_suffix("settings:retention:2", "settings:retention:"), Some(2));
        assert_eq!(parse_index_suffix("settings:retention:x", "settings:retention:"), None);
    }
}
