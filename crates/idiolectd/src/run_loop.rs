use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use idiolect_adapter_clipboard::ArboardClipboard;
use idiolect_adapter_crypto::{
    ChaCha20Poly1305Cipher, CryptoError, EncryptionKeyPort, EncryptionPort, FileKey,
};
use idiolect_adapter_ksni::{KsniTray, KsniTrayError, TrayCallback};
use idiolect_adapter_opus::{OpusCodec, OpusCodecError};
use idiolect_adapter_sqlite::{
    FileAudioStore, FileAudioStoreError, SqliteMetadataStore, SqliteStorageError,
};
use idiolect_adapter_vad::{VadAdapter, FRAME_DURATION_MS, FRAME_SAMPLE_COUNT};
use idiolect_application::use_cases::history::ClipboardPort;
use idiolect_application::use_cases::maintenance::{MaintenanceUseCase, DEFAULT_PRUNE_INTERVAL};
use idiolect_application::use_cases::menu::{
    validate_training_retention_days, MenuUseCase, RecordingState, MAX_ENTRY_CHOICES,
    RETENTION_DAY_CHOICES, TRAINING_RETENTION_CHOICES,
};
use idiolect_application::use_cases::segmentation::{
    FrameBuffer, SegmenterConfig, UtteranceSegmenter,
};
use idiolect_common::config::{HistoryConfig, TranslationConfig, VadConfig};
use idiolect_common::ids::ImeSessionId;
use idiolect_common::languages::is_supported_language;
use idiolect_ipc::framing::{decode_json_line, encode_json_line, FramingError};
use idiolect_ipc::handshake::{negotiate_protocol, HandshakeError};
use idiolect_ipc::messages::{
    CommitPreedit, ErrorMessage, HistoryCopyResponse, HistoryReinsertResponse, InsertText,
    IpcMessage, PreeditUpdate, RecordingStatus, FEATURE_RECORDING_STATUS,
};
use idiolect_ports::audio::AudioSegment;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioStorePort, MetadataStorePort, TrayIcon, TrayPort, TrayStatus};
use tokio::sync::watch;

use crate::adapters::{RuntimeAdapterError, RuntimeAdapterProfile};
use crate::retention_dialog::{RetentionDialog, SubprocessRetentionDialog};

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
    /// Translation defaults from the config file; the tray's translation toggle
    /// and language pickers persist overrides in `tray_settings`.
    pub(crate) translation_config: TranslationConfig,
    /// VAD timing rules; drives the pause-triggered segmenter in streaming mode.
    pub(crate) vad_config: VadConfig,
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
    let key_path = database_path.parent().map_or_else(
        || PathBuf::from("history.key"),
        |parent| parent.join("history.key"),
    );
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
    // A daemon that exited uncleanly can leave a stale socket file behind, which
    // makes `bind` fail with "Address already in use". Remove it first so a restart
    // succeeds — but ONLY if it is stale: if a live daemon is still listening, leave
    // it so `bind` fails and this second instance is correctly rejected.
    unlink_stale_socket(&config.socket_path);
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
    // Also purge training data (audio + transcript + candidate) past its (longer)
    // window on startup, so an idle daemon still bounds disk use.
    if effective.training_retention_days > 0 {
        let audio_store =
            FileAudioStore::new(config.audio_root.clone(), config.decoded_cache_root.clone());
        match store.prune_training_data(effective.training_retention_days, &audio_store) {
            Ok(removed) => eprintln!("startup training-data prune removed {removed} sessions"),
            Err(error) => eprintln!("startup training-data prune failed: {error}"),
        }
    }

    // A single tray owned here; its callbacks are drained inside the connection loop.
    let (tray_callback_tx, tray_callback_rx) = mpsc::channel::<TrayCallback>();
    let mut tray =
        KsniTray::new(tray_callback_tx).map_err(|error| RunLoopError::tray("tray init", error))?;
    // Degrade gracefully when there is no display (headless server, or a CI
    // runner without Xvfb): a missing system clipboard disables history copy but
    // must not stop the daemon — same policy as the tray above.
    let mut clipboard = match ArboardClipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            eprintln!("idiolect: clipboard unavailable, history copy disabled: {error}");
            ArboardClipboard::disabled()
        }
    };

    refresh_tray_menu(
        &mut tray,
        &store,
        &config.history_config,
        &config.translation_config,
        RecordingState::Idle,
    )?;
    tray.set_icon(TrayIcon::Idle)
        .map_err(|error| RunLoopError::tray("tray icon", error))?;
    tray.set_tooltip("Idiolect — Ready")
        .map_err(|error| RunLoopError::tray("tray tooltip", error))?;
    // Active (not Passive) so the tray host actually shows the icon — GNOME's
    // AppIndicator hides Passive items.
    tray.set_status(TrayStatus::Active)
        .map_err(|error| RunLoopError::tray("tray status", error))?;

    // Background pruning task on a dedicated thread with its own tokio runtime so
    // the loop is actually driven (a spawned task on an undriven runtime never runs).
    let (maintenance_shutdown_tx, maintenance_shutdown_rx) = watch::channel(());
    let maintenance_db = config.database_path.clone();
    let maintenance_audio_root = config.audio_root.clone();
    let maintenance_decoded_root = config.decoded_cache_root.clone();
    let maintenance_defaults = config.history_config.clone();
    let maintenance_handle = thread::Builder::new()
        .name("idiolect-maintenance".to_owned())
        .spawn(move || {
            run_maintenance(
                maintenance_db,
                maintenance_audio_root,
                maintenance_decoded_root,
                maintenance_defaults,
                maintenance_shutdown_rx,
            );
        })
        .map_err(|error| RunLoopError::io("spawn maintenance thread", error))?;

    let result = (|| loop {
        let (stream, _) = listener
            .accept()
            .map_err(|error| RunLoopError::io("accept client", error))?;
        handle_connection(
            stream,
            &config,
            &mut tray,
            &mut clipboard,
            &mut store,
            &tray_callback_rx,
        )?;
        if config.shutdown_after_client {
            return Ok(());
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
    audio_root: PathBuf,
    decoded_cache_root: PathBuf,
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
        let training_retention_days = config.training_retention_days;

        // History pruning runs through the tested use case; training-data pruning
        // needs the audio store (not part of the metadata port), so it runs as a
        // sibling loop on its own connection. Both honour the same shutdown.
        let history =
            MaintenanceUseCase::new(store, config, shutdown_rx.clone()).run_pruning_loop();
        let training = run_training_prune_loop(
            &database_path,
            &audio_root,
            &decoded_cache_root,
            training_retention_days,
            shutdown_rx,
        );
        let (history_result, ()) = tokio::join!(history, training);
        if let Err(error) = history_result {
            eprintln!("maintenance: pruning loop error: {error}");
        }
    });
}

/// Periodic training-data prune, mirroring the history loop's interval/shutdown
/// shape. `retention_days == 0` disables it (waits for shutdown).
async fn run_training_prune_loop(
    database_path: &Path,
    audio_root: &Path,
    decoded_cache_root: &Path,
    retention_days: u32,
    mut shutdown_rx: watch::Receiver<()>,
) {
    if retention_days == 0 {
        let _ = shutdown_rx.changed().await;
        return;
    }
    let mut ticker = tokio::time::interval(DEFAULT_PRUNE_INTERVAL);
    // The startup prune already ran once; consume the immediate first tick.
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                prune_training_data_once(database_path, audio_root, decoded_cache_root, retention_days);
            }
            _ = shutdown_rx.changed() => return,
        }
    }
}

/// Open a fresh store + audio store and purge training data older than
/// `older_than_days` once. Errors are logged, never propagated, so a single bad
/// pass never tears down maintenance. `older_than_days == 0` is a no-op.
fn prune_training_data_once(
    database_path: &Path,
    audio_root: &Path,
    decoded_cache_root: &Path,
    older_than_days: u32,
) {
    if older_than_days == 0 {
        return;
    }
    let mut store = match SqliteMetadataStore::open_path(database_path) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("training-data prune: open store failed: {error}");
            return;
        }
    };
    let audio_store = FileAudioStore::new(audio_root.to_owned(), decoded_cache_root.to_owned());
    match store.prune_training_data(older_than_days, &audio_store) {
        Ok(0) => {}
        Ok(removed) => eprintln!("training-data prune removed {removed} sessions"),
        Err(error) => eprintln!("training-data prune failed: {error}"),
    }
}

/// Resolves the active history configuration, layering persisted `tray_settings`
/// overrides on top of the config-file defaults. The `tray_settings` table is the
/// single source of truth at runtime.
fn effective_history_config(
    store: &SqliteMetadataStore,
    defaults: &HistoryConfig,
) -> HistoryConfig {
    let settings = store.get_all_tray_settings().unwrap_or_default();
    let retention_days = settings
        .get("retention_days")
        .and_then(|value| value.parse().ok())
        .unwrap_or(defaults.retention_days);
    let max_entries = settings
        .get("max_entries")
        .and_then(|value| value.parse().ok())
        .unwrap_or(defaults.max_entries);
    let training_retention_days = settings
        .get("training_retention_days")
        .and_then(|value| value.parse().ok())
        .unwrap_or(defaults.training_retention_days);
    HistoryConfig {
        retention_days,
        max_entries,
        training_retention_days,
        clipboard_auto_clear_secs: defaults.clipboard_auto_clear_secs,
        encrypt_at_rest: defaults.encrypt_at_rest,
    }
}

/// Resolves the active translation configuration, layering persisted
/// `tray_settings` overrides on top of the config-file defaults (the same
/// layering as [`effective_history_config`]). Language overrides are validated
/// against the catalogue so a stale or hand-edited value can never push an
/// unknown code into the pipeline; the external command stays config-file-only.
fn effective_translation_config(
    store: &SqliteMetadataStore,
    defaults: &TranslationConfig,
) -> TranslationConfig {
    let settings = store.get_all_tray_settings().unwrap_or_default();
    let enabled = settings
        .get("translation_enabled")
        .map(|value| value == "true")
        .unwrap_or(defaults.enabled);
    let input_language = settings
        .get("translation_input_lang")
        .filter(|code| *code == "auto" || is_supported_language(code))
        .cloned()
        .unwrap_or_else(|| defaults.input_language.clone());
    let output_language = settings
        .get("translation_output_lang")
        .filter(|code| is_supported_language(code))
        .cloned()
        .unwrap_or_else(|| defaults.output_language.clone());
    TranslationConfig {
        enabled,
        input_language,
        output_language,
        command: defaults.command.clone(),
    }
}

/// Whether "review before insert" mode is on (persisted in `tray_settings`).
/// In this mode the daemon flags each transcript so the client opens its own
/// review/correction dialog before committing, instead of inserting directly.
fn review_mode_enabled(store: &SqliteMetadataStore) -> bool {
    store
        .get_tray_setting("review_mode")
        .ok()
        .flatten()
        .as_deref()
        == Some("true")
}

/// Rebuilds and installs the tray menu from current storage state.
fn refresh_tray_menu(
    tray: &mut KsniTray,
    store: &SqliteMetadataStore,
    defaults: &HistoryConfig,
    translation_defaults: &TranslationConfig,
    recording_state: RecordingState,
) -> Result<(), RunLoopError> {
    let config = effective_history_config(store, defaults);
    let translation = effective_translation_config(store, translation_defaults);
    let entries = store
        .recent_history(config.max_entries)
        .map_err(|error| RunLoopError::storage("recent history", error))?;
    let mut menu = MenuUseCase::new().get_menu(recording_state, &entries, &config, &translation);
    menu.push(idiolect_ports::storage::TrayMenuItem {
        id: "review_mode".to_owned(),
        label: "Review before insert".to_owned(),
        enabled: true,
        kind: idiolect_ports::storage::TrayMenuItemKind::Checkable {
            checked: review_mode_enabled(store),
        },
    });
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
    // A short read timeout lets the loop wake periodically to service tray
    // callbacks (start/stop dictation from the menu) even when the client is
    // sending no IPC traffic.
    reader_stream
        .set_read_timeout(Some(Duration::from_millis(150)))
        .map_err(|error| RunLoopError::io("set read timeout", error))?;
    let mut reader = BufReader::new(reader_stream);
    let audio_store =
        FileAudioStore::new(config.audio_root.clone(), config.decoded_cache_root.clone());
    let codec = OpusCodec::new();
    // Out-of-process dialog for the "Custom…" retention entry; discovered once.
    let retention_dialog = SubprocessRetentionDialog::discover();
    // Per-connection state bundled into `Live`:
    //  - active_session: the in-flight dictation, if any.
    //  - live_capture: set only while a real microphone recording is in progress.
    //  - live_stream: set alongside live.live_capture while translation streams snippets.
    //  - status_tx: the authoritative recording-state publisher, re-armed at
    //    handshake once we know whether the client negotiated `recording_status`.
    let mut live = Live {
        active_session: None,
        live_capture: None,
        live_stream: None,
        status_tx: RecordingStatusTx::new(false),
    };
    let mut line = String::new();

    loop {
        // Drain any pending tray callbacks before (and between) IPC reads.
        while let Ok(callback) = tray_callback_rx.try_recv() {
            handle_tray_action(
                callback,
                &mut stream,
                tray,
                clipboard,
                Ctx {
                    store: &mut *store,
                    audio_store: &audio_store,
                    codec: &codec,
                    config,
                },
                &mut live,
                &retention_dialog,
            )?;
        }

        // While a streaming take is live, every loop tick (the 150 ms read
        // timeout guarantees one even with a silent client) pumps the mic
        // through the segmenter and delivers any pause-completed snippets.
        pump_live_stream(
            &mut stream,
            Ctx {
                store: &mut *store,
                audio_store: &audio_store,
                codec: &codec,
                config,
            },
            &mut live,
        )?;

        match reader.read_line(&mut line) {
            // A clean EOF (0) or an abrupt reset are both just the peer going away —
            // e.g. an IME engine restarting/reconnecting sends RST, not FIN. Treat
            // them identically: release the mic, cancel uncommitted work, and accept
            // the next client. Crashing the daemon on a client reset would let any
            // engine restart take the whole daemon down.
            Ok(0) => {
                drop_live_capture(&mut live.live_capture);
                cancel_uncommitted_active_session(
                    store,
                    &mut live.active_session,
                    "daemon-disconnect",
                )?;
                return Ok(());
            }
            Ok(_) => {}
            // A timeout just means no data yet; loop to re-check tray callbacks.
            // Any partial bytes already read stay buffered in `line`.
            Err(error) if is_read_timeout(&error) => {}
            Err(error) if is_disconnect(&error) => {
                drop_live_capture(&mut live.live_capture);
                cancel_uncommitted_active_session(
                    store,
                    &mut live.active_session,
                    "daemon-disconnect",
                )?;
                return Ok(());
            }
            Err(error) => return Err(RunLoopError::io("read ipc line", error)),
        }

        // Under a read timeout `read_line` can return on a partial read, so only
        // dispatch once a full newline-terminated line has accumulated.
        if !line.ends_with('\n') {
            continue;
        }
        let message = decode_json_line(&line).map_err(RunLoopError::framing)?;
        line.clear();

        match message {
            IpcMessage::ClientHello(client) => {
                let response = negotiate_protocol(&client).map_err(RunLoopError::handshake)?;
                let wants_status = response
                    .accepted_features
                    .iter()
                    .any(|feature| feature == FEATURE_RECORDING_STATUS);
                send_ipc_message(&mut stream, &IpcMessage::ServerHello(response))?;
                live.status_tx = RecordingStatusTx::new(wants_status);
                live.status_tx.sync_initial(&mut stream)?;
            }
            IpcMessage::StartRecording | IpcMessage::ToggleRecording => {
                if crate::adapters::is_live_capture(&config.adapter_profile) {
                    // Toggle: the same key starts, then stops and transcribes.
                    if live.live_capture.is_some() {
                        stop_live_and_transcribe(
                            &mut stream,
                            tray,
                            Ctx {
                                store: &mut *store,
                                audio_store: &audio_store,
                                codec: &codec,
                                config,
                            },
                            &mut live,
                        )?;
                    } else {
                        start_live_capture(
                            &mut stream,
                            tray,
                            Ctx {
                                store: &mut *store,
                                audio_store: &audio_store,
                                codec: &codec,
                                config,
                            },
                            &mut live,
                        )?;
                    }
                } else {
                    start_fixture_oneshot(
                        &mut stream,
                        tray,
                        Ctx {
                            store: &mut *store,
                            audio_store: &audio_store,
                            codec: &codec,
                            config,
                        },
                        &mut live,
                    )?;
                }
            }
            IpcMessage::StopRecording => {
                if live.live_capture.is_some() {
                    stop_live_and_transcribe(
                        &mut stream,
                        tray,
                        Ctx {
                            store: &mut *store,
                            audio_store: &audio_store,
                            codec: &codec,
                            config,
                        },
                        &mut live,
                    )?;
                }
            }
            IpcMessage::CommitPreedit(commit) => {
                commit_active_session(store, &mut live.active_session, commit)?;
                // A commit is a text event; the authoritative recording state is
                // whether the mic is still open. In streaming mode the engine
                // commits each snippet MID-recording — publishing a hardcoded
                // `false` here would flip the tray idle and tell the engine the
                // take ended, making it drop every later snippet.
                let recording = live.live_capture.is_some();
                live.status_tx
                    .set(&mut stream, tray, store, config, recording)?;
            }
            IpcMessage::ReportCorrection(correction) => {
                // The user fixed the auto-committed text in place: amend the
                // just-committed session with the corrected form, and re-render the
                // tray so the history entry shows the corrected text immediately.
                if let Some(active) = live.active_session.as_mut() {
                    if active.finalized {
                        store
                            .amend_correction(
                                active.session_id,
                                &active.current_text,
                                &correction.corrected_text,
                            )
                            .map_err(|error| RunLoopError::storage("amend correction", error))?;
                        active.current_text = correction.corrected_text.clone();
                        live.status_tx.refresh_tray(tray, store, config)?;
                    }
                }
            }
            IpcMessage::CancelPreedit => {
                drop_live_capture(&mut live.live_capture);
                live.live_stream = None;
                cancel_uncommitted_active_session(
                    store,
                    &mut live.active_session,
                    "daemon-cancel",
                )?;
                live.active_session = None;
                live.status_tx
                    .set(&mut stream, tray, store, config, false)?;
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
            IpcMessage::ServerHello(_)
            | IpcMessage::RecordingStatus(_)
            | IpcMessage::PreeditUpdate(_)
            | IpcMessage::InsertText(_)
            | IpcMessage::Error(_) => {
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

/// Per-recording streaming state for pause-triggered translation: incremental
/// resampling, frame re-chunking, frame-level VAD, and the utterance segmenter.
/// Created on start when translation is enabled; the pump feeds it every loop
/// tick and each finished snippet is transcribed/translated immediately.
struct LiveStreamState {
    /// Built lazily from the first non-empty poll, which reports the device's
    /// real capture rate (the config value is advisory; hardware decides).
    resampler: Option<crate::adapters::StreamingResampler>,
    frames: FrameBuffer,
    vad: VadAdapter,
    segmenter: UtteranceSegmenter,
}

impl LiveStreamState {
    fn new(vad_config: &VadConfig) -> Self {
        Self {
            resampler: None,
            frames: FrameBuffer::new(),
            vad: VadAdapter::new(),
            segmenter: UtteranceSegmenter::new(SegmenterConfig {
                sample_rate_hz: 16_000,
                frame_ms: FRAME_DURATION_MS as u32,
                min_speech_ms: vad_config.min_speech_ms,
                pre_roll_ms: vad_config.pre_roll_ms,
                post_roll_ms: vad_config.post_roll_ms,
                max_utterance_ms: vad_config.max_utterance_ms,
            }),
        }
    }

    /// Pushes one drained capture chunk through resample → frame → VAD →
    /// segmenter; returns the snippets (16 kHz mono samples) completed by it.
    fn ingest(&mut self, drained: &AudioSegment) -> Vec<Vec<f32>> {
        if drained.samples_f32_mono.is_empty() {
            return Vec::new();
        }
        let resampler = self.resampler.get_or_insert_with(|| {
            crate::adapters::StreamingResampler::new(drained.sample_rate_hz)
        });
        let resampled = resampler.push(&drained.samples_f32_mono);

        let mut snippets = Vec::new();
        for frame in self.frames.push(&resampled, FRAME_SAMPLE_COUNT) {
            // A frame the detector rejects is treated as silence: losing one
            // 30 ms verdict must never abort the whole take.
            let is_speech = self.vad.is_speech_frame(&frame).unwrap_or(false);
            if let Some(snippet) = self.segmenter.push_frame(&frame, is_speech) {
                snippets.push(snippet.samples_f32_mono);
            }
        }
        snippets
    }

    /// Recovers the un-paused tail utterance when recording stops.
    fn flush(&mut self) -> Option<Vec<f32>> {
        self.segmenter
            .flush()
            .map(|snippet| snippet.samples_f32_mono)
    }
}

enum StartSessionOutcome {
    Started(ActiveSession),
    Recoverable(RuntimeAdapterError),
}

/// One-shot fixture capture: the deterministic clip is produced and transcribed
/// immediately. Used by the fixture device profile (tests, CI).
fn start_fixture_session(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    codec: &OpusCodec,
    config: &RunLoopConfig,
) -> Result<StartSessionOutcome, RunLoopError> {
    let segment = match crate::adapters::finish_capture(crate::adapters::RuntimeCapture::Fixture) {
        Ok(segment) => segment,
        Err(error) => return Ok(StartSessionOutcome::Recoverable(error)),
    };
    materialize_session(store, audio_store, codec, config, segment)
}

/// Encodes, transcribes, and persists a captured audio segment into a new
/// dictation session. Shared by the fixture one-shot path and the live
/// stop-and-transcribe path so both produce identical session/audio state.
fn materialize_session(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    codec: &OpusCodec,
    config: &RunLoopConfig,
    segment: AudioSegment,
) -> Result<StartSessionOutcome, RunLoopError> {
    let encoded = codec
        .encode(&segment)
        .map_err(|error| RunLoopError::codec("encode audio", error))?;
    let decoded = codec
        .decode(&encoded)
        .map_err(|error| RunLoopError::codec("decode audio", error))?;
    let translation = effective_translation_config(store, &config.translation_config);
    let draft = match crate::adapters::transcribe_translated(
        &config.adapter_profile,
        &translation,
        &decoded,
    ) {
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

/// Stops and discards an in-progress live recording, releasing the microphone.
fn drop_live_capture(live_capture: &mut Option<crate::adapters::RuntimeCapture>) {
    if let Some(capture) = live_capture.take() {
        let _ = crate::adapters::finish_capture(capture);
    }
}

/// The single place recording-state changes are published. The daemon owns the
/// microphone, so it is the authority: every start/stop/cancel — whether driven by
/// the keyboard toggle over IPC or by a tray menu click — funnels through here,
/// which updates the tray icon/menu AND pushes [`IpcMessage::RecordingStatus`] to a
/// client that negotiated the feature. Because the tray and the push share this one
/// chokepoint, the keyboard, the tray, and the adapter indicator can never disagree.
///
/// Edge-triggered on the `recording` value: a no-op transition (e.g. a commit after
/// the mic already stopped) publishes nothing, so clients see one push per real change.
struct RecordingStatusTx {
    /// Whether the connected client negotiated `recording_status` pushes.
    feature: bool,
    /// The recording value last published to the tray (and, if `feature`, the client).
    last: bool,
}

impl RecordingStatusTx {
    fn new(feature: bool) -> Self {
        Self {
            feature,
            last: false,
        }
    }

    /// Sync the authoritative state to a client that just completed the handshake,
    /// so it starts mirrored even before anything happens.
    fn sync_initial(&self, stream: &mut UnixStream) -> Result<(), RunLoopError> {
        if self.feature {
            send_ipc_message(
                stream,
                &IpcMessage::RecordingStatus(RecordingStatus {
                    recording: self.last,
                }),
            )?;
        }
        Ok(())
    }

    /// Pure edge-trigger: record the transition and decide whether to push it over
    /// IPC. Only a real change (and a negotiated client) pushes — a repeated value
    /// (e.g. a commit after the stop already announced `false`) must not emit a
    /// duplicate `RecordingStatus`.
    fn should_push(&mut self, recording: bool) -> bool {
        let changed = recording != self.last;
        self.last = recording;
        changed && self.feature
    }

    /// Publish a recording-state value: ALWAYS refresh the tray, and push
    /// `RecordingStatus` over IPC only on a real transition.
    ///
    /// The tray refresh is deliberately not deduplicated on the recording value:
    /// the menu renders HISTORY, which changes on commit/correction/cancel even
    /// when the recording value does not. Skipping the refresh in that case made
    /// the tray history lag a full take behind (a real field bug).
    fn set(
        &mut self,
        stream: &mut UnixStream,
        tray: &mut KsniTray,
        store: &SqliteMetadataStore,
        config: &RunLoopConfig,
        recording: bool,
    ) -> Result<(), RunLoopError> {
        let state = if recording {
            RecordingState::Recording
        } else {
            RecordingState::Idle
        };
        update_tray_recording_state(tray, store, config, state)?;
        if self.should_push(recording) {
            send_ipc_message(
                stream,
                &IpcMessage::RecordingStatus(RecordingStatus { recording }),
            )?;
        }
        Ok(())
    }

    /// Re-render the tray (menu + icon) with the current recording value, pushing
    /// nothing. For history-only mutations (e.g. an in-place correction) that must
    /// show up in the menu immediately.
    fn refresh_tray(
        &self,
        tray: &mut KsniTray,
        store: &SqliteMetadataStore,
        config: &RunLoopConfig,
    ) -> Result<(), RunLoopError> {
        let state = if self.last {
            RecordingState::Recording
        } else {
            RecordingState::Idle
        };
        update_tray_recording_state(tray, store, config, state)
    }
}

/// Begins a live microphone recording. Emits nothing on success (the transcript
/// arrives on stop); reports an `Error` to the client if the device is
/// unavailable. The tray switches to the recording indicator.
///
/// When translation is enabled (config default or tray override), the take runs
/// in streaming mode: a [`LiveStreamState`] is armed and the pump delivers a
/// translated snippet at every pause instead of one transcript on stop.
/// Per-connection mutable state, bundled so the live-capture handlers stay under
/// the argument-count lint without threading many `&mut` params individually.
struct Live {
    active_session: Option<ActiveSession>,
    live_capture: Option<crate::adapters::RuntimeCapture>,
    live_stream: Option<LiveStreamState>,
    status_tx: RecordingStatusTx,
}

/// The long-lived store, codecs, and config a connection's handlers share.
/// Passed by value with reborrowed refs so callers keep using the originals.
struct Ctx<'a> {
    store: &'a mut SqliteMetadataStore,
    audio_store: &'a FileAudioStore,
    codec: &'a OpusCodec,
    config: &'a RunLoopConfig,
}

impl Ctx<'_> {
    /// A fresh `Ctx` borrowing the same data, so a handler can pass it on to a
    /// nested handler and keep using its own afterwards.
    fn reborrow(&mut self) -> Ctx<'_> {
        Ctx {
            store: &mut *self.store,
            audio_store: self.audio_store,
            codec: self.codec,
            config: self.config,
        }
    }
}

fn start_live_capture(
    stream: &mut UnixStream,
    tray: &mut KsniTray,
    ctx: Ctx<'_>,
    live: &mut Live,
) -> Result<(), RunLoopError> {
    let Ctx { store, config, .. } = ctx;
    let Live {
        active_session,
        live_capture,
        live_stream,
        status_tx,
    } = live;
    cancel_uncommitted_active_session(store, active_session, "daemon-retry")?;
    match crate::adapters::begin_capture(&config.adapter_profile) {
        Ok(capture) => {
            *live_capture = Some(capture);
            let translation = effective_translation_config(store, &config.translation_config);
            *live_stream = translation
                .enabled
                .then(|| LiveStreamState::new(&config.vad_config));
            status_tx.set(stream, tray, store, config, true)?;
        }
        Err(error) => {
            send_ipc_message(
                stream,
                &IpcMessage::Error(ErrorMessage {
                    code: error.code().to_owned(),
                    message: error.to_string(),
                }),
            )?;
        }
    }
    Ok(())
}

/// One pump tick of the streaming pipeline: drains whatever audio accumulated
/// since the last tick, advances the segmenter, and transcribes/translates and
/// delivers every snippet a pause just completed. A no-op outside streaming
/// takes. Poll failures are logged, never fatal — one bad tick must not end a
/// recording the user is mid-sentence in.
fn pump_live_stream(
    stream: &mut UnixStream,
    ctx: Ctx<'_>,
    live: &mut Live,
) -> Result<(), RunLoopError> {
    let Ctx {
        store,
        audio_store,
        codec,
        config,
    } = ctx;
    let Live {
        active_session,
        live_capture,
        live_stream,
        ..
    } = live;
    let (Some(capture), Some(state)) = (live_capture.as_mut(), live_stream.as_mut()) else {
        return Ok(());
    };
    let drained = match crate::adapters::poll_capture(capture) {
        Ok(segment) => segment,
        Err(error) => {
            eprintln!("live stream poll failed: {error}");
            return Ok(());
        }
    };
    for snippet in state.ingest(&drained) {
        deliver_snippet(
            stream,
            store,
            audio_store,
            codec,
            config,
            active_session,
            snippet,
        )?;
    }
    Ok(())
}

/// Transcribes/translates one pause-completed snippet, persists it as its own
/// session, and pushes it to the client as a `PreeditUpdate` (carrying the
/// review flag, so "review before insert" routes each snippet through the
/// client's dialog exactly like a stop-time transcript). A snippet that fails
/// to transcribe is logged and skipped — the take continues.
fn deliver_snippet(
    stream: &mut UnixStream,
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    codec: &OpusCodec,
    config: &RunLoopConfig,
    active_session: &mut Option<ActiveSession>,
    samples_f32_mono: Vec<f32>,
) -> Result<(), RunLoopError> {
    let duration_ms = (samples_f32_mono.len() as u64 * 1_000 / 16_000) as u32;
    let segment = AudioSegment {
        sample_rate_hz: 16_000,
        channels: 1,
        duration_ms,
        samples_f32_mono,
    };

    // The previous snippet is normally committed by the engine well before the
    // next pause completes (the pause threshold dwarfs the loop tick); if it
    // never was, close it out before it is displaced.
    cancel_uncommitted_active_session(store, active_session, "daemon-snippet-advance")?;
    match materialize_session(store, audio_store, codec, config, segment)? {
        StartSessionOutcome::Started(session) => {
            let text = session.current_text.clone();
            let review = review_mode_enabled(store);
            *active_session = Some(session);
            send_ipc_message(
                stream,
                &IpcMessage::PreeditUpdate(PreeditUpdate { text, review }),
            )?;
        }
        StartSessionOutcome::Recoverable(error) => {
            eprintln!("snippet transcription failed: {error}");
        }
    }
    Ok(())
}

/// Stops the live recording, transcribes the captured audio, and sends the
/// resulting preedit to the client. The tray stays in the recording state while
/// the preedit is pending (commit/cancel returns it to idle).
///
/// In streaming mode the snippets were already delivered while recording; the
/// stop only recovers the tail the user spoke after the last pause.
fn stop_live_and_transcribe(
    stream: &mut UnixStream,
    tray: &mut KsniTray,
    ctx: Ctx<'_>,
    live: &mut Live,
) -> Result<(), RunLoopError> {
    let Ctx {
        store,
        audio_store,
        codec,
        config,
    } = ctx;
    let Live {
        active_session,
        live_capture,
        live_stream,
        status_tx,
    } = live;
    let Some(capture) = live_capture.take() else {
        return Ok(());
    };

    if let Some(mut state) = live_stream.take() {
        // Streaming stop: drain the final capture chunk, flush the segmenter's
        // tail, and deliver whatever utterances remain. The whole take was
        // already consumed snippet by snippet — there is no batch transcription.
        match crate::adapters::finish_capture(capture) {
            Ok(tail) => {
                let mut snippets = state.ingest(&tail);
                snippets.extend(state.flush());
                for snippet in snippets {
                    deliver_snippet(
                        stream,
                        store,
                        audio_store,
                        codec,
                        config,
                        active_session,
                        snippet,
                    )?;
                }
            }
            Err(error) => {
                send_ipc_message(
                    stream,
                    &IpcMessage::Error(ErrorMessage {
                        code: error.code().to_owned(),
                        message: error.to_string(),
                    }),
                )?;
            }
        }
        status_tx.set(stream, tray, store, config, false)?;
        return Ok(());
    }

    let segment = match crate::adapters::finish_capture(capture) {
        Ok(segment) => segment,
        Err(error) => {
            send_ipc_message(
                stream,
                &IpcMessage::Error(ErrorMessage {
                    code: error.code().to_owned(),
                    message: error.to_string(),
                }),
            )?;
            status_tx.set(stream, tray, store, config, false)?;
            return Ok(());
        }
    };

    match materialize_session(store, audio_store, codec, config, segment)? {
        StartSessionOutcome::Started(session) => {
            let text = session.current_text.clone();
            let review = review_mode_enabled(store);
            *active_session = Some(session);
            send_ipc_message(
                stream,
                &IpcMessage::PreeditUpdate(PreeditUpdate { text, review }),
            )?;
            // The mic is closed once the take stops, so the authoritative state is
            // "not recording" even while the preedit is pending review/commit.
            status_tx.set(stream, tray, store, config, false)?;
        }
        StartSessionOutcome::Recoverable(error) => {
            send_ipc_message(
                stream,
                &IpcMessage::Error(ErrorMessage {
                    code: error.code().to_owned(),
                    message: error.to_string(),
                }),
            )?;
            status_tx.set(stream, tray, store, config, false)?;
        }
    }
    Ok(())
}

/// One-shot fixture dictation triggered by `StartRecording` on the fixture
/// device: capture + transcribe + preedit in a single step (unchanged behaviour).
fn start_fixture_oneshot(
    stream: &mut UnixStream,
    tray: &mut KsniTray,
    ctx: Ctx<'_>,
    live: &mut Live,
) -> Result<(), RunLoopError> {
    let Ctx {
        store,
        audio_store,
        codec,
        config,
    } = ctx;
    let Live {
        active_session,
        status_tx,
        ..
    } = live;
    cancel_uncommitted_active_session(store, active_session, "daemon-retry")?;
    match start_fixture_session(store, audio_store, codec, config)? {
        StartSessionOutcome::Started(session) => {
            let text = session.current_text.clone();
            let review = review_mode_enabled(store);
            *active_session = Some(session);
            send_ipc_message(
                stream,
                &IpcMessage::PreeditUpdate(PreeditUpdate { text, review }),
            )?;
            // A fixture one-shot captures and transcribes instantly, so the mic is
            // never held open: the authoritative state stays "not recording".
            status_tx.set(stream, tray, store, config, false)?;
        }
        StartSessionOutcome::Recoverable(error) => {
            send_ipc_message(
                stream,
                &IpcMessage::Error(ErrorMessage {
                    code: error.code().to_owned(),
                    message: error.to_string(),
                }),
            )?;
        }
    }
    Ok(())
}

/// Whether a failed socket read merely timed out (so the loop can re-check tray
/// callbacks) rather than being a real I/O error.
fn is_read_timeout(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Whether a failed read means the peer disconnected (cleanly or abruptly) rather
/// than a genuine daemon-side fault. A client that crashes or restarts resets the
/// connection (`ConnectionReset`/`BrokenPipe`/`ConnectionAborted`); the daemon must
/// treat that like EOF and move on, never crash.
fn is_disconnect(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ConnectionReset
            | ErrorKind::BrokenPipe
            | ErrorKind::ConnectionAborted
            | ErrorKind::UnexpectedEof
    )
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

/// Routes a tray activation. Recording controls (start/stop/cancel) need access
/// to the live capture and the IPC stream, so they are handled here; everything
/// else (history, settings) is delegated to [`handle_tray_callback`].
fn handle_tray_action(
    callback: TrayCallback,
    stream: &mut UnixStream,
    tray: &mut KsniTray,
    clipboard: &mut ArboardClipboard,
    mut ctx: Ctx<'_>,
    live: &mut Live,
    retention_dialog: &dyn RetentionDialog,
) -> Result<(), RunLoopError> {
    let TrayCallback::Activate(action) = callback;
    match action.as_str() {
        "start_recording" => {
            if crate::adapters::is_live_capture(&ctx.config.adapter_profile) {
                if live.live_capture.is_none() {
                    start_live_capture(stream, tray, ctx.reborrow(), live)?;
                }
            } else {
                start_fixture_oneshot(stream, tray, ctx.reborrow(), live)?;
            }
        }
        "stop_recording" => {
            if live.live_capture.is_some() {
                stop_live_and_transcribe(stream, tray, ctx.reborrow(), live)?;
            }
        }
        "cancel" => {
            drop_live_capture(&mut live.live_capture);
            live.live_stream = None;
            cancel_uncommitted_active_session(ctx.store, &mut live.active_session, "tray-cancel")?;
            live.active_session = None;
            live.status_tx
                .set(stream, tray, ctx.store, ctx.config, false)?;
        }
        // "Insert" must land where the cursor is, so it goes through the active
        // IME front-end (the connected engine on `stream`) which types it into
        // the focused app — unlike "Copy", which only stages the clipboard.
        action if parse_id_suffix(action, "insert:").is_some() => {
            let id = parse_id_suffix(action, "insert:").expect("checked by guard");
            insert_entry_via_ime(stream, ctx.store, id)?;
        }
        _ => handle_tray_callback(
            TrayCallback::Activate(action),
            tray,
            clipboard,
            ctx.store,
            ctx.config,
            retention_dialog,
        )?,
    }
    Ok(())
}

/// Re-insert a history entry by asking the active IME front-end to commit it at
/// the cursor. `stream` is the connected engine; if the entry is missing or the
/// send fails it is logged, never fatal.
fn insert_entry_via_ime(
    stream: &mut UnixStream,
    store: &SqliteMetadataStore,
    id: i64,
) -> Result<(), RunLoopError> {
    let entry = store
        .get_history_entry(id)
        .map_err(|error| RunLoopError::storage("get history entry", error))?;
    let Some(entry) = entry else {
        eprintln!("tray insert: history entry {id} not found");
        return Ok(());
    };
    send_ipc_message(
        stream,
        &IpcMessage::InsertText(InsertText { text: entry.text }),
    )
}

/// Persist a training-data retention value (in days) as the runtime override.
fn set_training_retention(store: &mut SqliteMetadataStore, days: u32) -> Result<(), RunLoopError> {
    store
        .set_tray_setting("training_retention_days", &days.to_string())
        .map_err(|error| RunLoopError::storage("set training_retention_days", error))
}

/// Applies a `translation:*` tray activation: the enable toggle or an
/// input/output language pick, persisted as `tray_settings` overrides. Returns
/// whether the action was a translation action (so the caller refreshes the
/// menu). Index parsing is total; an out-of-range index is logged and ignored.
fn apply_translation_tray_action(
    store: &mut SqliteMetadataStore,
    defaults: &TranslationConfig,
    action: &str,
) -> Result<bool, RunLoopError> {
    if action == "translation:enabled" {
        let enabled = effective_translation_config(store, defaults).enabled;
        let next = if enabled { "false" } else { "true" };
        store
            .set_tray_setting("translation_enabled", next)
            .map_err(|error| RunLoopError::storage("set translation_enabled", error))?;
        return Ok(true);
    }
    if let Some(index) = parse_index_suffix(action, "translation:input:") {
        match idiolect_application::use_cases::menu::translation_input_language_for_index(index) {
            Some(code) => store
                .set_tray_setting("translation_input_lang", code)
                .map_err(|error| RunLoopError::storage("set translation_input_lang", error))?,
            None => eprintln!("tray translation input index out of range: {index}"),
        }
        return Ok(true);
    }
    if let Some(index) = parse_index_suffix(action, "translation:output:") {
        match idiolect_application::use_cases::menu::translation_output_language_for_index(index) {
            Some(code) => store
                .set_tray_setting("translation_output_lang", code)
                .map_err(|error| RunLoopError::storage("set translation_output_lang", error))?,
            None => eprintln!("tray translation output index out of range: {index}"),
        }
        return Ok(true);
    }
    Ok(false)
}

/// Handles an activated tray menu item. Parsing is total — a malformed action id
/// is logged and ignored rather than panicking the daemon.
fn handle_tray_callback(
    callback: TrayCallback,
    tray: &mut KsniTray,
    clipboard: &mut ArboardClipboard,
    store: &mut SqliteMetadataStore,
    config: &RunLoopConfig,
    retention_dialog: &dyn RetentionDialog,
) -> Result<(), RunLoopError> {
    let TrayCallback::Activate(action) = callback;
    let defaults = &config.history_config;
    let translation_defaults = &config.translation_config;

    if action == "review_mode" {
        let next = if review_mode_enabled(store) {
            "false"
        } else {
            "true"
        };
        store
            .set_tray_setting("review_mode", next)
            .map_err(|error| RunLoopError::storage("set review_mode", error))?;
        refresh_tray_menu(
            tray,
            store,
            defaults,
            translation_defaults,
            RecordingState::Idle,
        )?;
    } else if apply_translation_tray_action(store, translation_defaults, &action)? {
        refresh_tray_menu(
            tray,
            store,
            defaults,
            translation_defaults,
            RecordingState::Idle,
        )?;
    } else if let Some(id) = parse_id_suffix(&action, "insert:") {
        let _ = reinsert_entry(store, clipboard, id, defaults.clipboard_auto_clear_secs)?;
    } else if let Some(id) = parse_id_suffix(&action, "copy:") {
        let _ = copy_entry(store, clipboard, id, defaults.clipboard_auto_clear_secs)?;
    } else if let Some(id) = parse_id_suffix(&action, "delete:") {
        match store.delete_history_entry(id) {
            Ok(()) => {
                refresh_tray_menu(
                    tray,
                    store,
                    defaults,
                    translation_defaults,
                    RecordingState::Idle,
                )?;
            }
            Err(error) => eprintln!("tray delete of entry {id} failed: {error}"),
        }
    } else if let Some(index) = parse_index_suffix(&action, "settings:retention:") {
        if let Some(days) = RETENTION_DAY_CHOICES.get(index) {
            store
                .set_tray_setting("retention_days", &days.to_string())
                .map_err(|error| RunLoopError::storage("set retention_days", error))?;
            refresh_tray_menu(
                tray,
                store,
                defaults,
                translation_defaults,
                RecordingState::Idle,
            )?;
        } else {
            eprintln!("tray retention index out of range: {index}");
        }
    } else if let Some(index) = parse_index_suffix(&action, "settings:max_entries:") {
        if let Some(max) = MAX_ENTRY_CHOICES.get(index) {
            store
                .set_tray_setting("max_entries", &max.to_string())
                .map_err(|error| RunLoopError::storage("set max_entries", error))?;
            refresh_tray_menu(
                tray,
                store,
                defaults,
                translation_defaults,
                RecordingState::Idle,
            )?;
        } else {
            eprintln!("tray max_entries index out of range: {index}");
        }
    } else if action == "settings:training_retention_custom" {
        // Open the custom-entry dialog (blocking — fine on the tray callback path);
        // a confirmed, in-range value is persisted, a cancel leaves things as-is.
        let current = effective_history_config(store, defaults).training_retention_days;
        if let Some(days) = retention_dialog.prompt_days(current) {
            match validate_training_retention_days(days) {
                Ok(()) => {
                    set_training_retention(store, days)?;
                    refresh_tray_menu(
                        tray,
                        store,
                        defaults,
                        translation_defaults,
                        RecordingState::Idle,
                    )?;
                }
                Err(error) => eprintln!("custom training retention rejected: {error}"),
            }
        }
    } else if let Some(index) = parse_index_suffix(&action, "settings:training_retention:") {
        // A preset; the appended "(custom)" marker has no preset and is a no-op.
        if let Some((_, days)) = TRAINING_RETENTION_CHOICES.get(index) {
            set_training_retention(store, *days)?;
            refresh_tray_menu(
                tray,
                store,
                defaults,
                translation_defaults,
                RecordingState::Idle,
            )?;
        }
    } else {
        // start_recording / stop_recording / cancel originate from the IME client
        // over IPC, not the tray; ignore anything else.
        eprintln!("unhandled tray action: {action}");
    }

    Ok(())
}

fn parse_id_suffix(action: &str, prefix: &str) -> Option<i64> {
    action
        .strip_prefix(prefix)
        .and_then(|rest| rest.parse().ok())
}

fn parse_index_suffix(action: &str, prefix: &str) -> Option<usize> {
    action
        .strip_prefix(prefix)
        .and_then(|rest| rest.parse().ok())
}

fn update_tray_recording_state(
    tray: &mut KsniTray,
    store: &SqliteMetadataStore,
    config: &RunLoopConfig,
    state: RecordingState,
) -> Result<(), RunLoopError> {
    refresh_tray_menu(
        tray,
        store,
        &config.history_config,
        &config.translation_config,
        state,
    )?;
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

/// Remove the socket file only if no live daemon is listening on it. Probing with
/// a connect distinguishes a stale leftover (connect refused → safe to unlink and
/// rebind) from a running instance (connect succeeds → leave it so `bind` fails and
/// the second instance is rejected). A missing file is a no-op.
fn unlink_stale_socket(path: &Path) {
    match UnixStream::connect(path) {
        Ok(_) => {} // a live daemon owns this socket — do not disturb it
        Err(_) => {
            let _ = fs::remove_file(path);
        }
    }
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
    use std::io::{Error, ErrorKind};

    use super::{
        is_disconnect, is_read_timeout, parse_id_suffix, parse_index_suffix,
        should_clear_clipboard, RecordingStatusTx,
    };

    #[test]
    fn clipboard_clears_only_when_unchanged() {
        assert!(should_clear_clipboard(Some("hello"), "hello"));
        assert!(!should_clear_clipboard(Some("changed"), "hello"));
        assert!(!should_clear_clipboard(None, "hello"));
    }

    #[test]
    fn read_timeout_recognises_only_timeout_errors() {
        assert!(is_read_timeout(&Error::from(ErrorKind::WouldBlock)));
        assert!(is_read_timeout(&Error::from(ErrorKind::TimedOut)));
        // A real I/O failure must not be mistaken for a poll timeout, or the
        // loop would spin instead of surfacing the error.
        assert!(!is_read_timeout(&Error::from(ErrorKind::ConnectionReset)));
        assert!(!is_read_timeout(&Error::from(ErrorKind::BrokenPipe)));
    }

    #[test]
    fn status_push_is_edge_triggered_and_feature_gated() {
        // The IPC push fires once per real transition. Repeating the same value —
        // e.g. a CommitPreedit after the stop already announced `false` — must not
        // emit a duplicate RecordingStatus. (The tray menu refresh is deliberately
        // NOT deduplicated this way: history changes on commit/correction even when
        // the recording value does not — that lag was a real field bug.)
        let mut tx = RecordingStatusTx::new(true);
        assert!(!tx.should_push(false), "initial false is not a transition");
        assert!(tx.should_push(true), "idle -> recording pushes");
        assert!(
            !tx.should_push(true),
            "repeated recording does not push again"
        );
        assert!(tx.should_push(false), "recording -> idle pushes");
        assert!(!tx.should_push(false), "commit after stop must not re-push");

        // Without the negotiated feature nothing is ever pushed, but transitions
        // are still tracked so a mid-session upgrade cannot replay stale state.
        let mut legacy = RecordingStatusTx::new(false);
        assert!(!legacy.should_push(true));
        assert!(!legacy.should_push(false));
    }

    #[test]
    fn disconnect_recognises_peer_resets_but_not_real_faults() {
        // A client that crashes or restarts resets the connection; the daemon must
        // treat that like EOF and accept the next client, never crash.
        assert!(is_disconnect(&Error::from(ErrorKind::ConnectionReset)));
        assert!(is_disconnect(&Error::from(ErrorKind::BrokenPipe)));
        assert!(is_disconnect(&Error::from(ErrorKind::ConnectionAborted)));
        assert!(is_disconnect(&Error::from(ErrorKind::UnexpectedEof)));
        // A genuine fault must still surface as an error, not be swallowed.
        assert!(!is_disconnect(&Error::from(ErrorKind::InvalidData)));
        assert!(!is_disconnect(&Error::from(ErrorKind::PermissionDenied)));
    }

    #[test]
    fn id_and_index_parsing_is_total() {
        assert_eq!(parse_id_suffix("delete:42", "delete:"), Some(42));
        assert_eq!(parse_id_suffix("delete:nan", "delete:"), None);
        assert_eq!(parse_id_suffix("copy:1", "delete:"), None);
        assert_eq!(
            parse_index_suffix("settings:retention:2", "settings:retention:"),
            Some(2)
        );
        assert_eq!(
            parse_index_suffix("settings:retention:x", "settings:retention:"),
            None
        );
    }

    mod insert_via_ime {
        use std::io::{BufRead, BufReader, ErrorKind, Read};
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        use idiolect_adapter_sqlite::SqliteMetadataStore;
        use idiolect_ipc::messages::IpcMessage;
        use idiolect_ports::storage::MetadataStorePort;

        use crate::run_loop::insert_entry_via_ime;

        /// Seed one committed history entry and return its store and row id.
        fn store_with_entry(text: &str) -> (SqliteMetadataStore, i64) {
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            store.migrate().expect("migrate");
            let session = store.create_session(Some(text)).expect("create");
            store
                .commit_session(session, text, "commit-1")
                .expect("commit");
            let id = store
                .recent_history(10)
                .expect("recent")
                .first()
                .expect("one entry")
                .id;
            (store, id)
        }

        #[test]
        fn insert_sends_the_entry_text_as_insert_text_to_the_engine() {
            // The fix: "Insert" must type the entry at the cursor via the IME,
            // *not* merely stage the clipboard like "Copy". The daemon proves it
            // by emitting an `InsertText` down the engine connection.
            let (store, id) = store_with_entry("Deploy traefik and nginx");
            let (engine_side, mut daemon_side) = UnixStream::pair().expect("socketpair");

            insert_entry_via_ime(&mut daemon_side, &store, id).expect("insert");
            // Close the daemon end so the read sees EOF after the message — a
            // *missing* message then fails cleanly instead of blocking forever.
            drop(daemon_side);

            let mut reader = BufReader::new(engine_side);
            let mut line = String::new();
            reader.read_line(&mut line).expect("read");
            assert!(
                !line.is_empty(),
                "Insert must send a message, not type nothing"
            );
            match idiolect_ipc::framing::decode_json_line(&line).expect("decode") {
                IpcMessage::InsertText(insert) => {
                    assert_eq!(insert.text, "Deploy traefik and nginx");
                }
                other => panic!("expected InsertText, got {other:?}"),
            }
        }

        #[test]
        fn insert_of_a_missing_entry_sends_nothing() {
            let (store, id) = store_with_entry("present");
            let (engine_side, mut daemon_side) = UnixStream::pair().expect("socketpair");

            insert_entry_via_ime(&mut daemon_side, &store, id + 999).expect("insert");

            // Nothing should have been written: a short read must time out rather
            // than yield bytes.
            engine_side
                .set_read_timeout(Some(Duration::from_millis(200)))
                .expect("timeout");
            let mut buf = [0u8; 1];
            match (&engine_side).read(&mut buf) {
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Ok(0) => {}
                other => panic!("expected no data for a missing entry, got {other:?}"),
            }
        }
    }

    mod training_prune {
        use std::path::PathBuf;

        use idiolect_adapter_sqlite::SqliteMetadataStore;

        use crate::run_loop::prune_training_data_once;

        fn temp_root(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir()
                .join(format!("idiolect-prune-once-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("temp root");
            dir
        }

        // Deletion semantics are covered by the storage integration test
        // (`training_retention.rs`); here we only confirm the maintenance wrapper
        // wires up safely — it opens the store, respects the disable switch, and
        // never panics on an empty database.
        #[test]
        fn prune_once_is_safe_on_empty_store_and_when_disabled() {
            let root = temp_root("noop");
            let db = root.join("idiolect.sqlite");
            let mut store = SqliteMetadataStore::open_path(&db).expect("open");
            store.migrate().expect("migrate");
            drop(store);

            let audio = root.join("audio");
            let decoded = root.join("decoded");
            // Enabled but no data → no-op; disabled (0) → no-op. Neither panics.
            prune_training_data_once(&db, &audio, &decoded, 365);
            prune_training_data_once(&db, &audio, &decoded, 0);

            let _ = std::fs::remove_dir_all(&root);
        }
    }

    mod live_stream_state {
        use idiolect_common::config::VadConfig;
        use idiolect_test_support::fixtures::speech_pause_speech_fixture_16khz_mono;

        use crate::run_loop::LiveStreamState;

        // The streaming pipeline glue (resample → frame → VAD → segmenter) on
        // the canned speech–pause–speech clip: exactly two utterances, both
        // completed by their pauses (no flush needed).
        #[test]
        fn speech_pause_speech_yields_two_snippets() {
            let mut state = LiveStreamState::new(&VadConfig::default());
            let clip = speech_pause_speech_fixture_16khz_mono();

            let snippets = state.ingest(&clip);

            assert_eq!(snippets.len(), 2, "one snippet per pause");
            for snippet in &snippets {
                // Each snippet must carry at least the spoken clip (~1s).
                assert!(
                    snippet.len() > 16_000 / 2,
                    "snippet too short: {}",
                    snippet.len()
                );
            }
            assert!(state.flush().is_none(), "no tail after the final pause");
        }

        // Ragged chunk delivery (like real 150 ms polls) must produce the same
        // outcome as one big drain.
        #[test]
        fn chunked_ingest_matches_one_shot() {
            let mut state = LiveStreamState::new(&VadConfig::default());
            let clip = speech_pause_speech_fixture_16khz_mono();

            let mut snippets = Vec::new();
            for chunk in clip.samples_f32_mono.chunks(2_400) {
                let segment = idiolect_ports::audio::AudioSegment {
                    sample_rate_hz: 16_000,
                    channels: 1,
                    duration_ms: (chunk.len() / 16) as u32,
                    samples_f32_mono: chunk.to_vec(),
                };
                snippets.extend(state.ingest(&segment));
            }

            assert_eq!(snippets.len(), 2);
        }
    }

    mod translation_tray_actions {
        use idiolect_adapter_sqlite::SqliteMetadataStore;
        use idiolect_common::config::TranslationConfig;
        use idiolect_common::languages::LANGUAGES;

        use crate::run_loop::{apply_translation_tray_action, effective_translation_config};

        fn store() -> SqliteMetadataStore {
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            store.migrate().expect("migrate");
            store
        }

        // The tray click itself goes through a StatusNotifier host (a GUI
        // boundary with no headless seam); this is the action logic it drives.
        #[test]
        fn toggle_flips_and_persists_translation_enabled() {
            let mut store = store();
            let defaults = TranslationConfig::default(); // disabled

            assert!(
                apply_translation_tray_action(&mut store, &defaults, "translation:enabled")
                    .expect("toggle")
            );
            assert!(effective_translation_config(&store, &defaults).enabled);

            assert!(
                apply_translation_tray_action(&mut store, &defaults, "translation:enabled")
                    .expect("toggle")
            );
            assert!(!effective_translation_config(&store, &defaults).enabled);
        }

        #[test]
        fn language_picks_map_indices_to_codes() {
            let mut store = store();
            let defaults = TranslationConfig::default();

            // Input index 0 is "Auto detect"; the rest follow the catalogue.
            let swedish = LANGUAGES
                .iter()
                .position(|(code, _)| *code == "sv")
                .expect("sv");
            assert!(apply_translation_tray_action(
                &mut store,
                &defaults,
                &format!("translation:input:{}", swedish + 1),
            )
            .expect("input pick"));
            let japanese = LANGUAGES
                .iter()
                .position(|(code, _)| *code == "ja")
                .expect("ja");
            assert!(apply_translation_tray_action(
                &mut store,
                &defaults,
                &format!("translation:output:{japanese}"),
            )
            .expect("output pick"));

            let effective = effective_translation_config(&store, &defaults);
            assert_eq!(effective.input_language, "sv");
            assert_eq!(effective.output_language, "ja");

            // Back to auto-detect via index 0.
            assert!(
                apply_translation_tray_action(&mut store, &defaults, "translation:input:0")
                    .expect("auto pick")
            );
            assert_eq!(
                effective_translation_config(&store, &defaults).input_language,
                "auto"
            );
        }

        #[test]
        fn out_of_range_and_foreign_actions_are_safe() {
            let mut store = store();
            let defaults = TranslationConfig::default();

            // Out-of-range indices are consumed (they ARE translation actions)
            // but change nothing.
            let oob = format!("translation:output:{}", LANGUAGES.len());
            assert!(apply_translation_tray_action(&mut store, &defaults, &oob).expect("oob"));
            assert_eq!(
                effective_translation_config(&store, &defaults).output_language,
                "en"
            );

            // Non-translation actions are left for the other handlers.
            assert!(
                !apply_translation_tray_action(&mut store, &defaults, "review_mode")
                    .expect("foreign")
            );
            assert!(
                !apply_translation_tray_action(&mut store, &defaults, "delete:3").expect("foreign")
            );
        }
    }

    mod translation_settings {
        use idiolect_adapter_sqlite::SqliteMetadataStore;
        use idiolect_common::config::TranslationConfig;
        use idiolect_ports::storage::MetadataStorePort;

        use crate::run_loop::effective_translation_config;

        // Mirrors the history-config layering: the config file provides defaults,
        // and per-setting tray overrides persisted in `tray_settings` win at
        // runtime. The tray click itself is a GUI boundary; this is the logic it
        // drives.
        #[test]
        fn tray_overrides_layer_over_config_defaults() {
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            store.migrate().expect("migrate");
            let defaults = TranslationConfig {
                enabled: false,
                input_language: "auto".to_owned(),
                output_language: "en".to_owned(),
                command: "/usr/bin/my-translator".to_owned(),
            };

            // No overrides → exactly the defaults.
            let effective = effective_translation_config(&store, &defaults);
            assert_eq!(effective, defaults);

            // The user flips translation on and picks Swedish → Japanese in the tray.
            store
                .set_tray_setting("translation_enabled", "true")
                .expect("set");
            store
                .set_tray_setting("translation_input_lang", "sv")
                .expect("set");
            store
                .set_tray_setting("translation_output_lang", "ja")
                .expect("set");

            let effective = effective_translation_config(&store, &defaults);
            assert!(effective.enabled);
            assert_eq!(effective.input_language, "sv");
            assert_eq!(effective.output_language, "ja");
            // The command is config-file-only (a path, not a menu choice).
            assert_eq!(effective.command, "/usr/bin/my-translator");
        }

        #[test]
        fn corrupt_overrides_fall_back_to_defaults() {
            // A hand-edited or stale setting must never poison the pipeline with
            // an unknown language code.
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            store.migrate().expect("migrate");
            let defaults = TranslationConfig::default();

            store
                .set_tray_setting("translation_input_lang", "klingon")
                .expect("set");
            store
                .set_tray_setting("translation_output_lang", "auto")
                .expect("set");

            let effective = effective_translation_config(&store, &defaults);
            assert_eq!(effective.input_language, "auto", "unknown input -> default");
            assert_eq!(
                effective.output_language, "en",
                "auto is invalid as output -> default"
            );
        }
    }

    mod training_retention_setting {
        use idiolect_adapter_sqlite::SqliteMetadataStore;
        use idiolect_application::use_cases::menu::TRAINING_RETENTION_CHOICES;
        use idiolect_common::config::HistoryConfig;

        use crate::run_loop::{effective_history_config, set_training_retention};

        // The tray click itself goes through a StatusNotifier host (a GUI boundary
        // with no headless seam, like the "Insert" action); here we test the logic
        // it drives — persisting a value and resolving it back as the runtime
        // override — which is what actually changes retention behaviour.
        #[test]
        fn preset_and_custom_values_persist_and_resolve() {
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            store.migrate().expect("migrate");
            let defaults = HistoryConfig::default();

            // Unset → falls back to the config default (one year).
            assert_eq!(
                effective_history_config(&store, &defaults).training_retention_days,
                365
            );

            // A preset (2 years) — the value the callback reads from the choices.
            let (_, two_years) = TRAINING_RETENTION_CHOICES[5];
            set_training_retention(&mut store, two_years).expect("set preset");
            assert_eq!(
                effective_history_config(&store, &defaults).training_retention_days,
                730
            );

            // A free-form custom value (from the dialog) overrides it.
            set_training_retention(&mut store, 540).expect("set custom");
            assert_eq!(
                effective_history_config(&store, &defaults).training_retention_days,
                540
            );
        }
    }
}
