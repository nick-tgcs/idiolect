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
use idiolect_adapter_vad::VadAdapter;
use idiolect_application::use_cases::history::ClipboardPort;
use idiolect_application::use_cases::maintenance::{MaintenanceUseCase, DEFAULT_PRUNE_INTERVAL};
use idiolect_application::use_cases::menu::{
    validate_training_retention_days, MenuUseCase, RecordingState, SyncMenuState,
    MAX_ENTRY_CHOICES, RETENTION_DAY_CHOICES, TRAINING_RETENTION_CHOICES,
};
use idiolect_application::use_cases::streaming::{
    merge_tail_correction, FinalizedTake, StreamObserver, StreamingConfig, StreamingTake,
    TakeOutcome, TakeTranscriber, TranscribeFailure,
};
use idiolect_common::config::{HistoryConfig, TranslationConfig, VadConfig};
use idiolect_common::ids::ImeSessionId;
use idiolect_common::languages::is_supported_language;
use idiolect_ipc::framing::{decode_json_line, encode_json_line, FramingError};
use idiolect_ipc::handshake::{negotiate_protocol, HandshakeError};
use idiolect_ipc::messages::{
    ActivityPhase, ActivityStatus, CommitPreedit, EditHistory, ErrorMessage, HistoryCopyResponse,
    HistoryReinsertResponse, InsertText, IpcMessage, PreeditUpdate, RecordingStatus,
    FEATURE_ACTIVITY_STATUS, FEATURE_RECONCILE, FEATURE_RECORDING_STATUS,
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
    /// The resolved data root, handed to the tray's dashboard subprocess so it
    /// operates on the daemon's store rather than its own default-path one.
    pub(crate) data_dir: PathBuf,
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
    /// Desktop-notification command for surfacing problems the user would
    /// otherwise never see (`<command> <summary> <body>`; empty = disabled).
    pub(crate) notify_command: String,
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

    /// Whether this error means the CLIENT went away mid-conversation — a peer
    /// reset surfacing on a daemon WRITE (`EPIPE`/`ECONNRESET`). The read path
    /// classifies resets inline (see [`is_disconnect`]); a failed write unwinds
    /// out of [`handle_connection`], so the accept loop uses this to survive it
    /// exactly like a read-side reset.
    pub(crate) fn is_client_disconnect(&self) -> bool {
        self.source
            .as_deref()
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .is_some_and(is_disconnect)
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

/// Where the at-rest history key lives: `history.key` beside the database.
/// ONE derivation, shared by the daemon's own cipher ([`build_history_cipher`])
/// and the store handed to the tray's dashboard ([`dashboard_store`]), so the
/// two processes can never cipher with different keys.
fn history_key_path(database_path: &Path) -> PathBuf {
    database_path.parent().map_or_else(
        || PathBuf::from("history.key"),
        |parent| parent.join("history.key"),
    )
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
    let key = FileKey::new(history_key_path(database_path))
        .load_or_create_key()
        .map_err(|error| RunLoopError::crypto("key load", error))?;
    Ok(Some(Box::new(ChaCha20Poly1305Cipher::new(key))))
}

/// The three out-of-process helpers the tray can open, discovered together.
///
/// Grouped so the daemon's notify command reaches all of them from ONE place.
/// Wired per-call-site, it is invisible when one of them silently loses the
/// ability to tell the user anything — the whole feature can be disabled by
/// three characters with every test still green.
pub(crate) struct HelperLaunchers {
    /// Backs the "Custom…" retention entry.
    pub(crate) retention_dialog: SubprocessRetentionDialog,
    /// Backs "Settings…".
    pub(crate) settings_window: crate::settings_launcher::SettingsLauncher,
    /// Backs "Corrections Dashboard…", handed the daemon's resolved store so
    /// pairing and training act on the database this daemon writes — not a
    /// second, default-path store.
    pub(crate) sync_panel: crate::sync_panel_launcher::SyncPanelLauncher,
}

impl HelperLaunchers {
    pub(crate) fn discover(config: &RunLoopConfig) -> Self {
        Self {
            retention_dialog: SubprocessRetentionDialog::discover(&config.notify_command),
            settings_window: crate::settings_launcher::SettingsLauncher::discover(
                &config.notify_command,
            ),
            sync_panel: crate::sync_panel_launcher::SyncPanelLauncher::discover(
                dashboard_store(config),
                &config.notify_command,
            ),
        }
    }
}

/// The store the tray hands to the dashboard subprocess: the daemon's resolved
/// paths, plus the history key exactly when this daemon ciphers with one
/// (`encrypt_at_rest` is config-file-only — `effective_history_config` never
/// overrides it — so this mirrors [`build_history_cipher`]'s decision).
fn dashboard_store(config: &RunLoopConfig) -> crate::sync_panel_launcher::DashboardStore {
    crate::sync_panel_launcher::DashboardStore {
        data_dir: config.data_dir.clone(),
        database_path: config.database_path.clone(),
        base_model: config.adapter_profile.whisper_model_path.clone(),
        history_key: config
            .history_config
            .encrypt_at_rest
            .then(|| history_key_path(&config.database_path)),
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
    // The Settings window feeds its changes into the SAME channel, so a change
    // made there is applied exactly like a tray click.
    let settings_forward_tx = tray_callback_tx.clone();
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

    refresh_tray_menu(&mut tray, &store, &config, RecordingState::Idle)?;
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
        // Per-connection state (see [`Live`]) — owned HERE so a write that
        // unwinds out of `handle_connection` can still be cleaned up below,
        // exactly like a read-side reset.
        let mut live = Live {
            active_session: None,
            live_capture: None,
            live_stream: None,
            status_tx: RecordingStatusTx::new(false, false),
            wants_reconcile: false,
        };
        match handle_connection(
            stream,
            &config,
            &mut tray,
            &mut clipboard,
            &mut store,
            &TrayChannel {
                rx: &tray_callback_rx,
                forward_tx: &settings_forward_tx,
            },
            &mut live,
        ) {
            Ok(()) => {}
            // A write racing the peer's reset (EPIPE mid-response) is the same
            // event as reading the reset: the client went away. The read path
            // handles it inside `handle_connection`; a failed WRITE unwinds out
            // of it, so it is converted here — release the mic, cancel
            // uncommitted work, repaint the tray idle, and accept the next
            // client. Crashing instead would let any engine restart take the
            // whole daemon down.
            Err(error) if error.is_client_disconnect() => {
                cleanup_disconnected_client(&mut tray, &mut store, &config, &mut live)?;
            }
            Err(error) => return Err(error),
        }
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

/// Resolves the active dictation-timing configuration, layering persisted
/// `tray_settings` overrides on top of the `[vad]` config-file defaults (the
/// same layering as history and translation). Unparseable overrides fall back
/// to the defaults, and a nonzero auto-stop below the pause threshold is lifted
/// to it so a take can never end before one snippet pause completes.
fn effective_vad_config(store: &SqliteMetadataStore, defaults: &VadConfig) -> VadConfig {
    let settings = store.get_all_tray_settings().unwrap_or_default();
    let override_ms = |key: &str, fallback: u32| {
        settings
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(fallback)
    };

    let post_roll_ms = override_ms("vad_post_roll_ms", defaults.post_roll_ms);
    let mut auto_stop_silence_ms =
        override_ms("vad_auto_stop_silence_ms", defaults.auto_stop_silence_ms);
    if auto_stop_silence_ms != 0 && auto_stop_silence_ms < post_roll_ms {
        auto_stop_silence_ms = post_roll_ms;
    }
    VadConfig {
        post_roll_ms,
        min_speech_ms: override_ms("vad_min_speech_ms", defaults.min_speech_ms),
        max_utterance_ms: override_ms("vad_max_utterance_ms", defaults.max_utterance_ms),
        auto_stop_silence_ms,
        ..defaults.clone()
    }
}

/// Applies a `settings:pause/min_speech/max_phrase/auto_stop:N` tray activation,
/// persisting the picked preset as a `tray_settings` override. Returns whether
/// the action was a dictation-timing action (so the caller refreshes the menu).
/// An out-of-range index (e.g. the appended "(custom)" marker) is consumed but
/// changes nothing.
fn apply_dictation_tray_action(
    store: &mut SqliteMetadataStore,
    action: &str,
) -> Result<bool, RunLoopError> {
    use idiolect_application::use_cases::menu::{
        auto_stop_ms_for_index, max_phrase_ms_for_index, min_speech_ms_for_index,
        pause_ms_for_index,
    };
    type MsForIndex = fn(usize) -> Option<u32>;
    let knobs: [(&str, &str, MsForIndex); 4] = [
        ("settings:pause:", "vad_post_roll_ms", pause_ms_for_index),
        (
            "settings:min_speech:",
            "vad_min_speech_ms",
            min_speech_ms_for_index,
        ),
        (
            "settings:max_phrase:",
            "vad_max_utterance_ms",
            max_phrase_ms_for_index,
        ),
        (
            "settings:auto_stop:",
            "vad_auto_stop_silence_ms",
            auto_stop_ms_for_index,
        ),
    ];
    for (prefix, key, value_for_index) in knobs {
        if let Some(index) = parse_index_suffix(action, prefix) {
            match value_for_index(index) {
                Some(ms) => store
                    .set_tray_setting(key, &ms.to_string())
                    .map_err(|error| RunLoopError::storage("set dictation timing", error))?,
                None => eprintln!("tray dictation-timing index out of range: {action}"),
            }
            return Ok(true);
        }
    }
    Ok(false)
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

/// Whether dictation types the live "preview" as you speak (persisted in
/// `tray_settings`, **default ON**). When on, streamed snippets are typed live and
/// the verified whole-take text replaces them at stop; when off, no snippets are
/// streamed and only the verified text is typed once, at stop. Absent ⇒ on, so the
/// feature is live for everyone without a migration.
fn preview_typing_enabled(store: &SqliteMetadataStore) -> bool {
    store
        .get_tray_setting("preview_typing")
        .ok()
        .flatten()
        .as_deref()
        != Some("false")
}

/// Flip the persisted "preview typing" setting (off ⇄ on). Mirrors the
/// review-mode toggle; the tray click / settings checkbox is the GUI boundary.
fn toggle_preview_typing(store: &mut SqliteMetadataStore) -> Result<(), RunLoopError> {
    let next = if preview_typing_enabled(store) {
        "false"
    } else {
        "true"
    };
    store
        .set_tray_setting("preview_typing", next)
        .map_err(|error| RunLoopError::storage("set preview_typing", error))
}

/// Rebuilds and installs the tray menu from current storage state.
///
/// Generic over [`TrayPort`] (the production tray is [`KsniTray`]) so tests can
/// observe renders through a recording fake — the real StatusNotifier host is a
/// GUI boundary.
fn refresh_tray_menu<T: TrayPort<Error = KsniTrayError>>(
    tray: &mut T,
    store: &SqliteMetadataStore,
    config: &RunLoopConfig,
    recording_state: RecordingState,
) -> Result<(), RunLoopError> {
    let history = effective_history_config(store, &config.history_config);
    let translation = effective_translation_config(store, &config.translation_config);
    let entries = store
        .recent_history(history.max_entries)
        .map_err(|error| RunLoopError::storage("recent history", error))?;
    let mut menu = MenuUseCase::new().get_menu(
        recording_state,
        &entries,
        &translation,
        &SyncMenuState::default(),
    );
    menu.push(idiolect_ports::storage::TrayMenuItem {
        id: "review_mode".to_owned(),
        label: "Review before insert".to_owned(),
        enabled: true,
        kind: idiolect_ports::storage::TrayMenuItemKind::Checkable {
            checked: review_mode_enabled(store),
        },
    });
    menu.push(idiolect_ports::storage::TrayMenuItem {
        id: "preview_typing".to_owned(),
        label: "Preview typing".to_owned(),
        enabled: true,
        kind: idiolect_ports::storage::TrayMenuItemKind::Checkable {
            checked: preview_typing_enabled(store),
        },
    });
    tray.set_menu(menu)
        .map_err(|error| RunLoopError::tray("tray menu", error))
}

/// The tray-callback channel pair a connection services: `rx` drains tray
/// clicks between IPC reads; `forward_tx` lets the Settings window feed its
/// changes back into the same stream.
struct TrayChannel<'a> {
    rx: &'a mpsc::Receiver<TrayCallback>,
    forward_tx: &'a mpsc::Sender<TrayCallback>,
}

fn handle_connection(
    mut stream: UnixStream,
    config: &RunLoopConfig,
    tray: &mut KsniTray,
    clipboard: &mut ArboardClipboard,
    store: &mut SqliteMetadataStore,
    tray_channel: &TrayChannel<'_>,
    live: &mut Live,
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
    let helpers = HelperLaunchers::discover(config);
    let mut line = String::new();

    loop {
        // Drain any pending tray callbacks before (and between) IPC reads.
        while let Ok(callback) = tray_channel.rx.try_recv() {
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
                live,
                &ConfigSurfaces {
                    retention_dialog: &helpers.retention_dialog,
                    settings_window: &helpers.settings_window,
                    sync_panel: &helpers.sync_panel,
                    settings_forward_tx: tray_channel.forward_tx,
                },
            )?;
        }

        // While a streaming take is live, every loop tick (the 150 ms read
        // timeout guarantees one even with a silent client) pumps the mic
        // through the segmenter and delivers any pause-completed snippets.
        let auto_stop = pump_live_stream(
            &mut stream,
            Ctx {
                store: &mut *store,
                audio_store: &audio_store,
                codec: &codec,
                config,
            },
            live,
        )?;
        if auto_stop {
            // The user went silent past the auto-stop threshold: the long pause
            // IS the stop — finalize the take exactly as a toggle would (one
            // review dialog / one committed session), and release the mic.
            stop_live_and_transcribe(
                &mut stream,
                tray,
                Ctx {
                    store: &mut *store,
                    audio_store: &audio_store,
                    codec: &codec,
                    config,
                },
                live,
            )?;
        }

        match reader.read_line(&mut line) {
            // A clean EOF (0) or an abrupt reset are both just the peer going away —
            // e.g. an IME engine restarting/reconnecting sends RST, not FIN. Treat
            // them identically: release the mic, cancel uncommitted work, repaint
            // the tray idle, and accept the next client. Crashing the daemon on a
            // client reset would let any engine restart take the whole daemon down.
            Ok(0) => {
                cleanup_disconnected_client(tray, store, config, live)?;
                return Ok(());
            }
            Ok(_) => {}
            // A timeout just means no data yet; loop to re-check tray callbacks.
            // Any partial bytes already read stay buffered in `line`.
            Err(error) if is_read_timeout(&error) => {}
            Err(error) if is_disconnect(&error) => {
                cleanup_disconnected_client(tray, store, config, live)?;
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
                live.wants_reconcile = response
                    .accepted_features
                    .iter()
                    .any(|feature| feature == FEATURE_RECONCILE);
                let wants_activity = response
                    .accepted_features
                    .iter()
                    .any(|feature| feature == FEATURE_ACTIVITY_STATUS);
                send_ipc_message(&mut stream, &IpcMessage::ServerHello(response))?;
                live.status_tx = RecordingStatusTx::new(wants_status, wants_activity);
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
                            live,
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
                            live,
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
                        live,
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
                        live,
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
                // For a streamed take the engine's correction window only ever
                // held the final snippet, so the correction replaces that tail of
                // the merged string rather than the whole take.
                if let Some(active) = live.active_session.as_mut() {
                    if active.finalized {
                        let corrected_full = merge_tail_correction(
                            &active.current_text,
                            active.tail_text.as_deref(),
                            &correction.corrected_text,
                        );
                        if corrected_full != active.current_text {
                            store
                                .amend_correction(
                                    active.session_id,
                                    &active.current_text,
                                    &corrected_full,
                                )
                                .map_err(|error| {
                                    RunLoopError::storage("amend correction", error)
                                })?;
                            active.current_text = corrected_full;
                            if active.tail_text.is_some() {
                                active.tail_text = Some(correction.corrected_text.clone());
                            }
                            live.status_tx.refresh_tray(tray, store, config);
                        }
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
            IpcMessage::HistoryEdited(edited) => {
                // The user retroactively corrected a past history entry via the
                // review dialog: amend the stored record and its raw→corrected
                // training pair. Any entry (not just the current take) may be
                // targeted by id; if the edited entry is the active session, keep
                // live state consistent so a later correction doesn't clobber it.
                match store.get_history_entry(edited.id) {
                    Ok(Some(entry)) => {
                        match apply_history_edit(store, edited.id, &edited.corrected_text) {
                            Ok(_) => {
                                if let Some(active) = live.active_session.as_mut() {
                                    if active.session_id == entry.session_id {
                                        active.current_text = edited.corrected_text.clone();
                                    }
                                }
                                live.status_tx.refresh_tray(tray, store, config);
                            }
                            Err(error) => {
                                eprintln!("history edit: amend failed: {error}");
                            }
                        }
                    }
                    Ok(None) => {
                        eprintln!("history edit: entry {} not found", edited.id);
                    }
                    Err(error) => {
                        eprintln!("history edit: lookup failed: {error}");
                    }
                }
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
            | IpcMessage::ActivityStatus(_)
            | IpcMessage::PreeditUpdate(_)
            | IpcMessage::InsertText(_)
            | IpcMessage::EditHistory(_)
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
    /// For a streamed take committed daemon-side: the final snippet's text.
    /// The engine's post-commit correction window only ever tracks that last
    /// snippet, so an incoming correction replaces this suffix of
    /// `current_text`, not the whole take.
    tail_text: Option<String>,
}

/// Per-recording streaming state for pause-triggered dictation. The take's
/// segmenter, accumulators, auto-stop clock, and error de-duplication live in the
/// shared [`StreamingTake`] orchestration (so the Android path runs the identical
/// rules); the daemon keeps only the two desktop-specific feeds — the lazy
/// capture-rate [`crate::adapters::StreamingResampler`] and the [`VadAdapter`] —
/// and pipes resampled 16 kHz audio plus each frame's speech verdict into it.
struct LiveStreamState {
    /// Built lazily from the first non-empty poll, which reports the device's
    /// real capture rate (the config value is advisory; hardware decides).
    resampler: Option<crate::adapters::StreamingResampler>,
    vad: VadAdapter,
    take: StreamingTake,
}

impl LiveStreamState {
    fn new(vad_config: &VadConfig) -> Self {
        Self {
            resampler: None,
            vad: VadAdapter::new(),
            take: StreamingTake::new(&StreamingConfig {
                min_speech_ms: vad_config.min_speech_ms,
                pre_roll_ms: vad_config.pre_roll_ms,
                post_roll_ms: vad_config.post_roll_ms,
                max_utterance_ms: vad_config.max_utterance_ms,
                auto_stop_silence_ms: vad_config.auto_stop_silence_ms,
            }),
        }
    }

    /// Resamples one drained capture chunk to 16 kHz mono and pushes it through
    /// the take's segmenter, labelling each frame with the VAD; returns the
    /// snippets a pause completed.
    fn ingest(&mut self, drained: &AudioSegment) -> Vec<Vec<f32>> {
        if drained.samples_f32_mono.is_empty() {
            return Vec::new();
        }
        let Self {
            resampler,
            vad,
            take,
        } = self;
        let resampled = resampler
            .get_or_insert_with(|| crate::adapters::StreamingResampler::new(drained.sample_rate_hz))
            .push(&drained.samples_f32_mono);
        take.ingest(&resampled, |frame| {
            vad.is_speech_frame(frame).unwrap_or(false)
        })
    }

    /// Recovers the un-paused tail utterance when recording stops.
    fn flush(&mut self) -> Option<Vec<f32>> {
        self.take.flush()
    }

    /// Whether the take has gone silent past its auto-stop threshold.
    fn auto_stop_due(&self) -> bool {
        self.take.auto_stop_due()
    }
}

/// Binds the daemon's transcribe+translate to the take's decode port: builds a
/// 16 kHz segment and runs `transcribe_translated`, re-reading the effective
/// translation config each call so a tray toggle mid-take takes effect on the
/// next snippet.
struct DaemonTranscriber<'a> {
    store: &'a SqliteMetadataStore,
    config: &'a RunLoopConfig,
}

impl TakeTranscriber for DaemonTranscriber<'_> {
    fn transcribe(&mut self, samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure> {
        let duration_ms = (samples_f32_mono.len() as u64 * 1_000 / 16_000) as u32;
        let segment = AudioSegment {
            sample_rate_hz: 16_000,
            channels: 1,
            duration_ms,
            samples_f32_mono: samples_f32_mono.to_vec(),
        };
        let translation = effective_translation_config(self.store, &self.config.translation_config);
        crate::adapters::transcribe_translated(&self.config.adapter_profile, &translation, &segment)
            .map(|draft| draft.text)
            .map_err(|error| TranscribeFailure {
                code: error.code().to_owned(),
                message: error.to_string(),
            })
    }
}

/// Routes a live take's events to the IPC client and the desktop notifier. Each
/// snippet is pushed as a PARTIAL preedit — typed by the engine in direct mode,
/// or display-only when "review before insert" is on; a failed snippet surfaces
/// once per take as a desktop notification.
struct DaemonObserver<'a> {
    stream: &'a mut UnixStream,
    store: &'a SqliteMetadataStore,
    config: &'a RunLoopConfig,
    /// Whether the client negotiated [`FEATURE_RECONCILE`]. Preview typing can only
    /// be suppressed for such a client (it gets the verified text at stop); a
    /// client without reconcile must always receive live snippets or nothing would
    /// ever be typed.
    wants_reconcile: bool,
}

impl StreamObserver for DaemonObserver<'_> {
    type Error = RunLoopError;

    fn snippet_committed(&mut self, chunk: &str) -> Result<(), RunLoopError> {
        // "Preview typing" off: type nothing live — the verified whole-take text is
        // sent once, at stop, as the reconcile final. Only do this for a client that
        // negotiated reconcile (else it would receive nothing); review mode always
        // streams display-only snippets into its dialog regardless of the toggle.
        let review = review_mode_enabled(self.store);
        if !review && self.wants_reconcile && !preview_typing_enabled(self.store) {
            return Ok(());
        }
        send_ipc_message(
            self.stream,
            &IpcMessage::PreeditUpdate(PreeditUpdate {
                text: chunk.to_owned(),
                review,
                partial: true,
                reconcile: false,
            }),
        )
    }

    fn snippet_dropped(&mut self, decoded: &str) -> Result<(), RunLoopError> {
        eprintln!(
            "snippet decode dropped ({decoded:?}); its audio is kept for the stop-time decode"
        );
        Ok(())
    }

    fn transcribe_failed(&mut self, code: &str, message: &str) -> Result<(), RunLoopError> {
        eprintln!("snippet transcription failed: {message}");
        // The journal alone is invisible: the user pauses, nothing appears, and
        // they can't tell broken from working. Tell them — once per take.
        let mut body = message.to_owned();
        if code == "translation-unavailable" {
            body.push_str(
                "\nSet translation.command in config.toml, or switch \
                 'Translate to' back to English in the tray.",
            );
        }
        crate::adapters::notify_user(
            &self.config.notify_command,
            "Idiolect — dictation is failing",
            &body,
        );
        Ok(())
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
    let session_id = persist_session(store, audio_store, &config.user_id, &encoded, &draft.text)?;

    Ok(StartSessionOutcome::Started(ActiveSession {
        session_id,
        current_text: draft.text,
        finalized: false,
        tail_text: None,
    }))
}

/// Creates the session row and stores its source audio: the persistence half of
/// [`materialize_session`], also used by the streaming path where the text is
/// already known (accumulated snippet by snippet) and must not be re-derived.
fn persist_session(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    user_id: &str,
    encoded: &idiolect_ports::audio::EncodedAudio,
    text: &str,
) -> Result<ImeSessionId, RunLoopError> {
    let session_id = store
        .create_session(Some(text))
        .map_err(|error| RunLoopError::storage("create session", error))?;
    let utterance_id = idiolect_common::ids::utterance_id_for_session(session_id);
    audio_store
        .write_source_audio(user_id, &utterance_id, encoded)
        .map_err(|error| RunLoopError::audio_store("write source audio", error))?;
    // Content digest of the encoded payload. The trainer's manifest builder
    // rejects an empty digest, so without this real captures could never be
    // validated/trained — historically the column was only ever set in tests.
    let audio_digest = idiolect_common::digest::audio_sha256_hex(&encoded.payload);
    store
        .set_audio_digest(&utterance_id, &audio_digest)
        .map_err(|error| RunLoopError::storage("set audio digest", error))?;
    Ok(session_id)
}

/// Stops and discards an in-progress live recording, releasing the microphone.
fn drop_live_capture(live_capture: &mut Option<crate::adapters::RuntimeCapture>) {
    if let Some(capture) = live_capture.take() {
        let _ = crate::adapters::finish_capture(capture);
    }
}

/// The single unwind path for a client that vanished — whether the reset
/// surfaced on a READ (EOF/ECONNRESET in the read loop) or on a WRITE (EPIPE
/// unwinding out of [`handle_connection`]): release the microphone, cancel
/// any uncommitted session, and re-publish Idle to the tray.
///
/// The tray render must not be skipped: the client may have died after
/// `start_live_capture` rendered Recording, and nothing else repaints the tray
/// before the next take. It also must not touch the dead stream, so it goes
/// through [`repaint_tray`] directly, never `status_tx.set` — and best-effort,
/// because a failed repaint here would hand the accept loop a fatal error and
/// turn a routine client disconnect into a daemon exit.
fn cleanup_disconnected_client<T: TrayPort<Error = KsniTrayError>>(
    tray: &mut T,
    store: &mut SqliteMetadataStore,
    config: &RunLoopConfig,
    live: &mut Live,
) -> Result<(), RunLoopError> {
    drop_live_capture(&mut live.live_capture);
    cancel_uncommitted_active_session(store, &mut live.active_session, "daemon-disconnect")?;
    repaint_tray(tray, store, config, RecordingState::Idle);
    Ok(())
}

/// The single place recording-state changes are published TO A LIVE CLIENT. The
/// daemon owns the microphone, so it is the authority: every start/stop/cancel —
/// whether driven by the keyboard toggle over IPC or by a tray menu click — funnels
/// through here, which updates the tray icon/menu AND pushes
/// [`IpcMessage::RecordingStatus`] to a client that negotiated the feature. Because
/// the tray and the push share this one chokepoint, the keyboard, the tray, and the
/// adapter indicator can never disagree. The one path that bypasses it is
/// [`cleanup_disconnected_client`]: with the client gone there is nothing to push
/// to, so it repaints the tray directly.
///
/// Edge-triggered on the `recording` value: a no-op transition (e.g. a commit after
/// the mic already stopped) publishes nothing, so clients see one push per real change.
struct RecordingStatusTx {
    /// Whether the connected client negotiated `recording_status` pushes.
    feature: bool,
    /// Whether the connected client negotiated `activity_status` pushes.
    activity_feature: bool,
    /// The phase last published to the tray (and, per feature, to the client).
    last: ActivityPhase,
}

/// What one phase transition puts on the wire. Two channels, because they answer
/// different questions: `recording` is the authoritative "is a take in flight"
/// the engine's state machine runs on, `activity` is the finer phase the overlay
/// shows the user. The decode moves the second without moving the first.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct StatusPush {
    recording: Option<bool>,
    activity: Option<ActivityPhase>,
}

impl RecordingStatusTx {
    fn new(feature: bool, activity_feature: bool) -> Self {
        Self {
            feature,
            activity_feature,
            last: ActivityPhase::Idle,
        }
    }

    /// Sync the authoritative state to a client that just completed the handshake,
    /// so it starts mirrored even before anything happens.
    fn sync_initial(&self, stream: &mut UnixStream) -> Result<(), RunLoopError> {
        if self.feature {
            send_ipc_message(
                stream,
                &IpcMessage::RecordingStatus(RecordingStatus {
                    recording: self.last.take_in_flight(),
                }),
            )?;
        }
        if self.activity_feature {
            send_ipc_message(
                stream,
                &IpcMessage::ActivityStatus(ActivityStatus { phase: self.last }),
            )?;
        }
        Ok(())
    }

    /// Pure edge-trigger: record the transition and decide what to push over IPC.
    /// Only a real change (and a negotiated client) pushes — a repeated value
    /// (e.g. a commit after the stop already announced idle) must not emit a
    /// duplicate. `RecordingStatus` additionally only moves when the take actually
    /// starts or ends, so the decode phase leaves it alone.
    fn should_push(&mut self, phase: ActivityPhase) -> StatusPush {
        let previous = std::mem::replace(&mut self.last, phase);
        if phase == previous {
            return StatusPush::default();
        }
        let in_flight = phase.take_in_flight();
        StatusPush {
            recording: (self.feature && in_flight != previous.take_in_flight())
                .then_some(in_flight),
            activity: self.activity_feature.then_some(phase),
        }
    }

    /// Publish a recording-state value: ALWAYS refresh the tray, and push over IPC
    /// only on a real transition.
    ///
    /// The tray refresh is deliberately not deduplicated on the recording value:
    /// the menu renders HISTORY, which changes on commit/correction/cancel even
    /// when the recording value does not. Skipping the refresh in that case made
    /// the tray history lag a full take behind (a real field bug).
    fn set<T: TrayPort<Error = KsniTrayError>>(
        &mut self,
        stream: &mut UnixStream,
        tray: &mut T,
        store: &SqliteMetadataStore,
        config: &RunLoopConfig,
        recording: bool,
    ) -> Result<(), RunLoopError> {
        let phase = if recording {
            ActivityPhase::Recording
        } else {
            ActivityPhase::Idle
        };
        self.set_phase(stream, tray, store, config, phase)
    }

    /// As [`Self::set`], but naming the exact phase — used by the stop path to
    /// announce the decode, which holds the take open without holding the
    /// microphone open.
    fn set_phase<T: TrayPort<Error = KsniTrayError>>(
        &mut self,
        stream: &mut UnixStream,
        tray: &mut T,
        store: &SqliteMetadataStore,
        config: &RunLoopConfig,
        phase: ActivityPhase,
    ) -> Result<(), RunLoopError> {
        let state = if phase.take_in_flight() {
            RecordingState::Recording
        } else {
            RecordingState::Idle
        };
        repaint_tray(tray, store, config, state);
        let push = self.should_push(phase);
        // Authoritative state first, presentation second: a client that mirrors
        // `RecordingStatus` must never act on a phase for a take it has not been
        // told about.
        if let Some(recording) = push.recording {
            send_ipc_message(
                stream,
                &IpcMessage::RecordingStatus(RecordingStatus { recording }),
            )?;
        }
        if let Some(phase) = push.activity {
            send_ipc_message(
                stream,
                &IpcMessage::ActivityStatus(ActivityStatus { phase }),
            )?;
        }
        Ok(())
    }

    /// Re-render the tray (menu + icon) with the current recording value, pushing
    /// nothing. For history-only mutations (e.g. an in-place correction) that must
    /// show up in the menu immediately.
    fn refresh_tray<T: TrayPort<Error = KsniTrayError>>(
        &self,
        tray: &mut T,
        store: &SqliteMetadataStore,
        config: &RunLoopConfig,
    ) {
        let state = if self.last.take_in_flight() {
            RecordingState::Recording
        } else {
            RecordingState::Idle
        };
        repaint_tray(tray, store, config, state);
    }
}

/// Tell the client which decode phase the take has entered. Call only once the
/// microphone has been stopped AND drained, so the badge never claims the mic is
/// shut while it is still collecting samples.
fn announce_decode_phase(
    stream: &mut UnixStream,
    tray: &mut KsniTray,
    store: &SqliteMetadataStore,
    config: &RunLoopConfig,
    status_tx: &mut RecordingStatusTx,
) -> Result<(), RunLoopError> {
    let phase = decode_phase(crate::adapters::asr_model_is_resident(
        &config.adapter_profile,
    ));
    status_tx.set_phase(stream, tray, store, config, phase)
}

/// Which decode phase to announce, given whether the speech model is already in
/// memory. A cold start pays a multi-second model load before any decoding
/// happens; calling that "transcribing" makes an ordinary wait look like a hang.
fn decode_phase(model_is_resident: bool) -> ActivityPhase {
    if model_is_resident {
        ActivityPhase::Transcribing
    } else {
        ActivityPhase::LoadingModel
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
/// Owned by the accept loop in [`run`] — not by [`handle_connection`] — so a
/// write that unwinds out of the connection can still be cleaned up like a
/// read-side reset.
struct Live {
    /// The in-flight dictation session, if any.
    active_session: Option<ActiveSession>,
    /// Set only while a real microphone recording is in progress.
    live_capture: Option<crate::adapters::RuntimeCapture>,
    /// Set alongside `live_capture` while translation streams snippets.
    live_stream: Option<LiveStreamState>,
    /// The authoritative recording-state publisher, re-armed at handshake once
    /// we know whether the client negotiated `recording_status`.
    status_tx: RecordingStatusTx,
    /// Whether the connected client negotiated [`FEATURE_RECONCILE`]. Only such a
    /// client receives the stop-time reconcile final (and may have its live
    /// snippets suppressed when preview typing is off); clients that did not
    /// advertise it keep the pre-reconcile behaviour, so they can never mistake
    /// the reconcile final for an appended batch transcript.
    wants_reconcile: bool,
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
        wants_reconcile: _,
    } = live;
    cancel_uncommitted_active_session(store, active_session, "daemon-retry")?;
    match crate::adapters::begin_capture(&config.adapter_profile) {
        Ok(capture) => {
            *live_capture = Some(capture);
            // Every live take streams: pause-triggered snippets (plain or
            // translated) and silence auto-stop are the default behaviour.
            // Timing comes from the effective config (tray overrides layered on
            // the file), captured at arm time so a take's rules never shift
            // under it mid-recording.
            *live_stream = Some(LiveStreamState::new(&effective_vad_config(
                store,
                &config.vad_config,
            )));
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
/// since the last tick, advances the segmenter, and transcribes/translates
/// every snippet a pause just completed, folding it into the take. A no-op
/// outside streaming takes. Poll failures are logged, never fatal — one bad
/// tick must not end a recording the user is mid-sentence in.
///
/// Returns `true` when the take has gone silent past
/// `vad.auto_stop_silence_ms`: the caller then stops the take exactly as a
/// toggle would — the long pause IS the stop.
fn pump_live_stream(
    stream: &mut UnixStream,
    ctx: Ctx<'_>,
    live: &mut Live,
) -> Result<bool, RunLoopError> {
    let Ctx { store, config, .. } = ctx;
    let Live {
        live_capture,
        live_stream,
        wants_reconcile,
        ..
    } = live;
    let wants_reconcile = *wants_reconcile;
    let (Some(capture), Some(state)) = (live_capture.as_mut(), live_stream.as_mut()) else {
        return Ok(false);
    };
    let drained = match crate::adapters::poll_capture(capture) {
        Ok(segment) => segment,
        Err(error) => {
            eprintln!("live stream poll failed: {error}");
            return Ok(false);
        }
    };
    for snippet in state.ingest(&drained) {
        fold_snippet_into_take(
            stream,
            store,
            config,
            &mut state.take,
            snippet,
            wants_reconcile,
        )?;
    }
    Ok(state.auto_stop_due())
}

/// Decodes one pause-completed snippet through the shared [`StreamingTake`],
/// binding the daemon's transcribe+translate and the IPC/notify feeds. The
/// orchestration folds the audio and text and emits the PARTIAL preedit (typed
/// in direct mode, display-only under review) or the once-per-take failure.
fn fold_snippet_into_take(
    stream: &mut UnixStream,
    store: &mut SqliteMetadataStore,
    config: &RunLoopConfig,
    take: &mut StreamingTake,
    snippet: Vec<f32>,
    wants_reconcile: bool,
) -> Result<(), RunLoopError> {
    let mut transcriber = DaemonTranscriber {
        store: &*store,
        config,
    };
    let mut observer = DaemonObserver {
        stream,
        store: &*store,
        config,
        wants_reconcile,
    };
    take.fold_snippet(&mut transcriber, &mut observer, snippet)
}

/// Closes out a streamed take as ONE session: the shared [`StreamingTake`]
/// decodes the merged recording once as a whole (the authoritative text, falling
/// back to the glued snippet previews if that decode fails) and reports the
/// outcome; the daemon then persists it. With review off the preview text already
/// reached the app via partials, so the session is committed daemon-side; with
/// review on, the final text goes to the client once, as the single review
/// dialog. An empty take (no speech) stores nothing.
fn finalize_streamed_take(
    stream: &mut UnixStream,
    ctx: Ctx<'_>,
    active_session: &mut Option<ActiveSession>,
    state: LiveStreamState,
    wants_reconcile: bool,
) -> Result<(), RunLoopError> {
    let Ctx {
        store,
        audio_store,
        codec,
        config,
    } = ctx;

    let outcome = {
        let mut transcriber = DaemonTranscriber {
            store: &*store,
            config,
        };
        state.take.finalize(&mut transcriber)
    };
    let FinalizedTake {
        final_text,
        merged_samples,
        // Only used in the non-reconcile fallback, where the engine's correction
        // window opens over the take's final snippet (the pre-reconcile contract).
        last_snippet_text,
        fallback_reason,
    } = match outcome {
        TakeOutcome::Silent => return Ok(()),
        TakeOutcome::Speech(finalized) => finalized,
    };
    if let Some(reason) = fallback_reason {
        eprintln!(
            "whole-take transcription failed at stop; keeping the previewed snippet text: {reason}"
        );
    }

    let duration_ms = (merged_samples.len() as u64 * 1_000 / 16_000) as u32;
    let segment = AudioSegment {
        sample_rate_hz: 16_000,
        channels: 1,
        duration_ms,
        samples_f32_mono: merged_samples,
    };
    let encoded = codec
        .encode(&segment)
        .map_err(|error| RunLoopError::codec("encode audio", error))?;

    cancel_uncommitted_active_session(store, active_session, "daemon-retry")?;
    let session_id = persist_session(store, audio_store, &config.user_id, &encoded, &final_text)?;

    if review_mode_enabled(store) {
        *active_session = Some(ActiveSession {
            session_id,
            current_text: final_text.clone(),
            finalized: false,
            tail_text: None,
        });
        send_ipc_message(
            stream,
            &IpcMessage::PreeditUpdate(PreeditUpdate {
                text: final_text,
                review: true,
                partial: false,
                reconcile: false,
            }),
        )?;
    } else {
        let key = idempotency_key("daemon-stream-final", session_id)?;
        store
            .commit_session(session_id, &final_text, &key)
            .map_err(|error| RunLoopError::storage("commit streamed take", error))?;
        // Direct mode: the daemon owns and has just committed the streamed session.
        let tail_text = if wants_reconcile {
            // Send the verified whole-take text so the engine REPLACES the preview
            // it typed live (backspace + re-commit) — or, with preview typing off,
            // types it fresh. The in-place correction window then covers the FULL
            // text, so `tail_text` is None (a later fix amends the whole take; see
            // the ReportCorrection handler's None branch).
            send_ipc_message(
                stream,
                &IpcMessage::PreeditUpdate(PreeditUpdate {
                    text: final_text.clone(),
                    review: false,
                    partial: false,
                    reconcile: true,
                }),
            )?;
            None
        } else {
            // The client did not negotiate reconcile: keep the pre-reconcile
            // behaviour — send NO stop-time final (an older engine would append it
            // after the preview it already typed) and let its correction window
            // open over the take's final snippet. The preview it typed stands.
            last_snippet_text
        };
        *active_session = Some(ActiveSession {
            session_id,
            current_text: final_text,
            finalized: true,
            tail_text,
        });
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
        wants_reconcile,
    } = live;
    let wants_reconcile = *wants_reconcile;
    let Some(capture) = live_capture.take() else {
        return Ok(());
    };

    // NOTE: the microphone is still OPEN here. `live_capture.take()` only moves
    // the capture out of `Live`; the stream keeps collecting until
    // `finish_capture` stops and drains it. So the decode phase is announced
    // below, once per branch, immediately after that call — announcing it here
    // would tell the user the mic had closed while it was still recording them,
    // and fold whatever they said during the tray/IPC round-trip into the take.
    if let Some(mut state) = live_stream.take() {
        // Streaming stop: drain the final capture chunk, flush the segmenter's
        // tail, fold the remaining utterances into the take, then finalize the
        // WHOLE take as one session — there is no batch transcription.
        match crate::adapters::finish_capture(capture) {
            Ok(tail) => {
                // Microphone now stopped and drained; everything below decodes,
                // which on CPU is seconds of apparent nothing. `recording`
                // deliberately stays true — the transcript is still to come and
                // the engine only accepts one while its take is open.
                announce_decode_phase(stream, tray, store, config, status_tx)?;
                let mut snippets = state.ingest(&tail);
                snippets.extend(state.flush());
                for snippet in snippets {
                    fold_snippet_into_take(
                        stream,
                        store,
                        config,
                        &mut state.take,
                        snippet,
                        wants_reconcile,
                    )?;
                }
                finalize_streamed_take(
                    stream,
                    Ctx {
                        store,
                        audio_store,
                        codec,
                        config,
                    },
                    active_session,
                    state,
                    wants_reconcile,
                )?;
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

    // As above: only now is the microphone actually stopped and drained.
    announce_decode_phase(stream, tray, store, config, status_tx)?;

    match materialize_session(store, audio_store, codec, config, segment)? {
        StartSessionOutcome::Started(session) => {
            let text = session.current_text.clone();
            let review = review_mode_enabled(store);
            *active_session = Some(session);
            send_ipc_message(
                stream,
                &IpcMessage::PreeditUpdate(PreeditUpdate {
                    text,
                    review,
                    partial: false,
                    reconcile: false,
                }),
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
                &IpcMessage::PreeditUpdate(PreeditUpdate {
                    text,
                    review,
                    partial: false,
                    reconcile: false,
                }),
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
    if active.finalized {
        // Already closed out — e.g. a streamed take the daemon committed
        // itself. A late engine-side CommitPreedit (or a retry) must not
        // re-commit under a different key or clobber the merged text.
        return Ok(());
    }

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
/// The out-of-process configuration surfaces a tray action can open: the
/// "Custom…" retention prompt and the Settings window (plus the channel the
/// window's changes flow back through).
struct ConfigSurfaces<'a> {
    retention_dialog: &'a dyn RetentionDialog,
    settings_window: &'a crate::settings_launcher::SettingsLauncher,
    sync_panel: &'a crate::sync_panel_launcher::SyncPanelLauncher,
    settings_forward_tx: &'a mpsc::Sender<TrayCallback>,
}

fn handle_tray_action(
    callback: TrayCallback,
    stream: &mut UnixStream,
    tray: &mut KsniTray,
    clipboard: &mut ArboardClipboard,
    mut ctx: Ctx<'_>,
    live: &mut Live,
    surfaces: &ConfigSurfaces<'_>,
) -> Result<(), RunLoopError> {
    let TrayCallback::Activate(action) = callback;
    match action.as_str() {
        "settings:open" => {
            surfaces.settings_window.open(
                settings_state_json(ctx.store, ctx.config),
                surfaces.settings_forward_tx.clone(),
            );
        }
        "open:dashboard" => {
            surfaces
                .sync_panel
                .open(surfaces.settings_forward_tx.clone());
        }
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
        // "Edit…" opens the review dialog over a past history entry so the user
        // can fix it; the result comes back as `HistoryEdited` (engine→daemon).
        action if parse_id_suffix(action, "edit:").is_some() => {
            let id = parse_id_suffix(action, "edit:").expect("checked by guard");
            edit_entry_via_ime(stream, ctx.store, id)?;
        }
        _ => handle_tray_callback(
            TrayCallback::Activate(action),
            tray,
            clipboard,
            ctx.store,
            ctx.config,
            surfaces.retention_dialog,
        )?,
    }
    Ok(())
}

/// The current effective settings, serialized for the Settings window's stdin
/// (its one input line). Effective = config-file defaults with `tray_settings`
/// overrides applied — exactly what the daemon will dictate with next take.
fn settings_state_json(store: &SqliteMetadataStore, config: &RunLoopConfig) -> String {
    let history = effective_history_config(store, &config.history_config);
    let translation = effective_translation_config(store, &config.translation_config);
    let vad = effective_vad_config(store, &config.vad_config);
    serde_json::json!({
        "pause_ms": vad.post_roll_ms,
        "min_speech_ms": vad.min_speech_ms,
        "max_phrase_ms": vad.max_utterance_ms,
        "auto_stop_ms": vad.auto_stop_silence_ms,
        "review_mode": review_mode_enabled(store),
        "preview_typing": preview_typing_enabled(store),
        "translation_enabled": translation.enabled,
        "input_lang": translation.input_language,
        "output_lang": translation.output_language,
        "translator_configured": !translation.command.is_empty(),
        "retention_days": history.retention_days,
        "max_entries": history.max_entries,
        "training_retention_days": history.training_retention_days,
    })
    .to_string()
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

/// Open the review dialog over a stored history entry so the user can fix a past
/// take without typing anything into the active app. `stream` is the connected
/// engine; if the entry is missing or the send fails it is logged, never fatal.
fn edit_entry_via_ime(
    stream: &mut UnixStream,
    store: &SqliteMetadataStore,
    id: i64,
) -> Result<(), RunLoopError> {
    let entry = store
        .get_history_entry(id)
        .map_err(|error| RunLoopError::storage("get history entry", error))?;
    let Some(entry) = entry else {
        eprintln!("tray edit: history entry {id} not found");
        return Ok(());
    };
    send_ipc_message(
        stream,
        &IpcMessage::EditHistory(EditHistory {
            id,
            text: entry.text,
        }),
    )
}

/// Look up history entry `id` and amend its stored record and raw→corrected
/// training pair with `corrected_text`. Returns `Ok(true)` if the entry was
/// found and amended, `Ok(false)` if it was not found (non-fatal).
fn apply_history_edit(
    store: &mut SqliteMetadataStore,
    id: i64,
    corrected_text: &str,
) -> Result<bool, RunLoopError> {
    let entry = store
        .get_history_entry(id)
        .map_err(|error| RunLoopError::storage("get history entry", error))?;
    let Some(entry) = entry else {
        return Ok(false);
    };
    store
        .amend_correction(entry.session_id, &entry.text, corrected_text)
        .map_err(|error| RunLoopError::storage("amend history edit", error))?;
    Ok(true)
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
        refresh_tray_menu(tray, store, config, RecordingState::Idle)?;
    } else if action == "preview_typing" {
        toggle_preview_typing(store)?;
        refresh_tray_menu(tray, store, config, RecordingState::Idle)?;
    } else if apply_translation_tray_action(store, translation_defaults, &action)?
        || apply_dictation_tray_action(store, &action)?
    {
        refresh_tray_menu(tray, store, config, RecordingState::Idle)?;
    } else if let Some(id) = parse_id_suffix(&action, "insert:") {
        let _ = reinsert_entry(store, clipboard, id, defaults.clipboard_auto_clear_secs)?;
    } else if let Some(id) = parse_id_suffix(&action, "copy:") {
        let _ = copy_entry(store, clipboard, id, defaults.clipboard_auto_clear_secs)?;
    } else if let Some(id) = parse_id_suffix(&action, "delete:") {
        match store.delete_history_entry(id) {
            Ok(()) => {
                refresh_tray_menu(tray, store, config, RecordingState::Idle)?;
            }
            Err(error) => eprintln!("tray delete of entry {id} failed: {error}"),
        }
    } else if let Some(index) = parse_index_suffix(&action, "settings:retention:") {
        if let Some(days) = RETENTION_DAY_CHOICES.get(index) {
            store
                .set_tray_setting("retention_days", &days.to_string())
                .map_err(|error| RunLoopError::storage("set retention_days", error))?;
            refresh_tray_menu(tray, store, config, RecordingState::Idle)?;
        } else {
            eprintln!("tray retention index out of range: {index}");
        }
    } else if let Some(index) = parse_index_suffix(&action, "settings:max_entries:") {
        if let Some(max) = MAX_ENTRY_CHOICES.get(index) {
            store
                .set_tray_setting("max_entries", &max.to_string())
                .map_err(|error| RunLoopError::storage("set max_entries", error))?;
            refresh_tray_menu(tray, store, config, RecordingState::Idle)?;
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
                    refresh_tray_menu(tray, store, config, RecordingState::Idle)?;
                }
                Err(error) => eprintln!("custom training retention rejected: {error}"),
            }
        }
    } else if let Some(index) = parse_index_suffix(&action, "settings:training_retention:") {
        // A preset; the appended "(custom)" marker has no preset and is a no-op.
        if let Some((_, days)) = TRAINING_RETENTION_CHOICES.get(index) {
            set_training_retention(store, *days)?;
            refresh_tray_menu(tray, store, config, RecordingState::Idle)?;
        }
    } else if let Some(days) = parse_index_suffix(&action, "settings:training_retention_days:") {
        // A direct day count (the Settings window's free-form field — it has a
        // real input box, so no prompt dialog is needed). Validated like the
        // dialog path; out-of-range values are logged and ignored.
        let days = u32::try_from(days).unwrap_or(u32::MAX);
        match validate_training_retention_days(days) {
            Ok(()) => {
                set_training_retention(store, days)?;
                refresh_tray_menu(tray, store, config, RecordingState::Idle)?;
            }
            Err(error) => eprintln!("settings training retention rejected: {error}"),
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

/// Repaint the tray for `state`, reporting a failure instead of propagating it.
///
/// Every caller is publishing PRESENTATION, and two of them do it while a take
/// exists only in memory: [`announce_decode_phase`] runs after `finish_capture`
/// has drained the microphone but before the audio is folded or finalized. The
/// menu renders `recent_history`, so this touches SQLite, which fails
/// transiently whenever another process (the trainer, the sync server,
/// trainerctl) has the database busy. Propagating that error there skipped the
/// finalize AND — the accept loop treating any non-disconnect error as fatal —
/// stopped the daemon, so a momentarily busy database destroyed the take the
/// user had just spoken. A repaint is never worth a take.
///
/// Deliberately NOT applied at startup (`run`) or to the tray-callback handlers:
/// no take is in flight there, so failing fast still surfaces a broken tray
/// without costing anything.
fn repaint_tray<T: TrayPort<Error = KsniTrayError>>(
    tray: &mut T,
    store: &SqliteMetadataStore,
    config: &RunLoopConfig,
    state: RecordingState,
) {
    if let Err(error) = update_tray_recording_state(tray, store, config, state) {
        eprintln!("tray: repaint failed, continuing: {error}");
    }
}

fn update_tray_recording_state<T: TrayPort<Error = KsniTrayError>>(
    tray: &mut T,
    store: &SqliteMetadataStore,
    config: &RunLoopConfig,
    state: RecordingState,
) -> Result<(), RunLoopError> {
    refresh_tray_menu(tray, store, config, state)?;
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

    use idiolect_ipc::messages::IpcMessage;

    use super::{
        decode_phase, is_disconnect, is_read_timeout, parse_id_suffix, parse_index_suffix,
        send_ipc_message, should_clear_clipboard, RecordingStatusTx, RunLoopError, StatusPush,
    };
    use idiolect_ipc::messages::ActivityPhase;

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
        let mut tx = RecordingStatusTx::new(true, false);
        let recording = |push: StatusPush| push.recording;
        assert_eq!(
            recording(tx.should_push(ActivityPhase::Idle)),
            None,
            "initial idle is not a transition"
        );
        assert_eq!(
            recording(tx.should_push(ActivityPhase::Recording)),
            Some(true),
            "idle -> recording pushes"
        );
        assert_eq!(
            recording(tx.should_push(ActivityPhase::Recording)),
            None,
            "repeated recording does not push again"
        );
        assert_eq!(
            recording(tx.should_push(ActivityPhase::Idle)),
            Some(false),
            "recording -> idle pushes"
        );
        assert_eq!(
            recording(tx.should_push(ActivityPhase::Idle)),
            None,
            "commit after stop must not re-push"
        );

        // Without the negotiated feature nothing is ever pushed, but transitions
        // are still tracked so a mid-session upgrade cannot replay stale state.
        let mut legacy = RecordingStatusTx::new(false, false);
        assert_eq!(
            legacy.should_push(ActivityPhase::Recording),
            StatusPush::default()
        );
        assert_eq!(
            legacy.should_push(ActivityPhase::Idle),
            StatusPush::default()
        );
    }

    #[test]
    fn the_decode_phase_pushes_activity_without_disturbing_recording() {
        // The crux of the caret indicator. At stop the microphone is shut but the
        // decode has not run, so the take is still in flight: `recording` MUST
        // stay true across it (the engine discards a transcript that arrives
        // after `recording = false`), while the phase changes so the overlay can
        // stop showing a live-microphone badge and show progress instead.
        let mut tx = RecordingStatusTx::new(true, true);
        assert_eq!(
            tx.should_push(ActivityPhase::Recording),
            StatusPush {
                recording: Some(true),
                activity: Some(ActivityPhase::Recording),
            },
        );
        assert_eq!(
            tx.should_push(ActivityPhase::Transcribing),
            StatusPush {
                recording: None,
                activity: Some(ActivityPhase::Transcribing),
            },
            "the decode must not re-announce the recording state",
        );
        assert_eq!(
            tx.should_push(ActivityPhase::Idle),
            StatusPush {
                recording: Some(false),
                activity: Some(ActivityPhase::Idle),
            },
        );
    }

    #[test]
    fn a_client_without_the_activity_feature_sees_the_old_byte_stream() {
        // Strictly additive: an engine that predates the phase channel must get
        // exactly the messages it always did, in the same places.
        let mut legacy = RecordingStatusTx::new(true, false);
        assert_eq!(
            legacy.should_push(ActivityPhase::Recording),
            StatusPush {
                recording: Some(true),
                activity: None,
            },
        );
        assert_eq!(
            legacy.should_push(ActivityPhase::Transcribing),
            StatusPush::default(),
            "the decode phase is invisible to a client that did not ask for it",
        );
        assert_eq!(
            legacy.should_push(ActivityPhase::Idle),
            StatusPush {
                recording: Some(false),
                activity: None,
            },
        );
    }

    #[test]
    fn a_repeated_phase_pushes_nothing() {
        // Edge-triggered on the phase, so a re-publish of the same state (a
        // commit after the stop already announced idle) stays silent.
        let mut tx = RecordingStatusTx::new(true, true);
        assert_eq!(tx.should_push(ActivityPhase::Idle), StatusPush::default());
        assert!(tx.should_push(ActivityPhase::Recording).activity.is_some());
        assert_eq!(
            tx.should_push(ActivityPhase::Recording),
            StatusPush::default(),
        );
    }

    #[test]
    fn a_cold_take_announces_the_model_load_rather_than_a_slow_decode() {
        // The user's original complaint: the first take after an idle day takes
        // many seconds because the model is being read off disk. Loading and
        // decoding must be distinguishable, or the overlay can only say
        // "transcribing" for twenty seconds and look hung.
        assert_eq!(decode_phase(false), ActivityPhase::LoadingModel);
        assert_eq!(decode_phase(true), ActivityPhase::Transcribing);
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
    fn a_write_to_a_reset_client_classifies_as_client_disconnect() {
        // The reset can surface on the daemon's WRITE (EPIPE) instead of its
        // read when responses race the peer's close — the daemon must classify
        // that as the client going away, not as a daemon fault.
        let (mut daemon_side, engine_side) =
            std::os::unix::net::UnixStream::pair().expect("socketpair");
        drop(engine_side);
        // The first write can land in the kernel buffer before the reset is
        // processed; keep writing until the failure surfaces.
        let error = loop {
            if let Err(error) = send_ipc_message(&mut daemon_side, &IpcMessage::StartRecording) {
                break error;
            }
        };
        assert!(
            error.is_client_disconnect(),
            "EPIPE on an IPC write means the client went away: {error}"
        );
    }

    #[test]
    fn only_peer_disconnect_io_errors_classify_as_client_disconnect() {
        let fault = RunLoopError::io("write ipc line", Error::from(ErrorKind::PermissionDenied));
        assert!(
            !fault.is_client_disconnect(),
            "a genuine io fault must stay fatal"
        );
        let sourceless = RunLoopError {
            message: "no source".to_owned(),
            source: None,
        };
        assert!(!sourceless.is_client_disconnect());
    }

    #[test]
    fn id_and_index_parsing_is_total() {
        assert_eq!(parse_id_suffix("delete:42", "delete:"), Some(42));
        assert_eq!(parse_id_suffix("delete:nan", "delete:"), None);
        assert_eq!(parse_id_suffix("copy:1", "delete:"), None);
        assert_eq!(parse_id_suffix("edit:7", "edit:"), Some(7));
        assert_eq!(
            parse_index_suffix("settings:retention:2", "settings:retention:"),
            Some(2)
        );
        assert_eq!(
            parse_index_suffix("settings:retention:x", "settings:retention:"),
            None
        );
    }

    // The pure streaming-take text logic (`snippet_chunk`, `choose_final_take_text`,
    // `merge_tail_correction`, `is_noise_transcript`) moved to
    // `idiolect_application::use_cases::streaming` (M2) and is unit-tested there;
    // the daemon's streaming integration tests below still exercise it end to end.

    mod disconnect_cleanup {
        use std::path::PathBuf;

        use idiolect_adapter_ksni::KsniTrayError;
        use idiolect_adapter_sqlite::SqliteMetadataStore;
        use idiolect_common::config::{HistoryConfig, TranslationConfig, VadConfig};
        use idiolect_ipc::messages::ActivityPhase;
        use idiolect_ports::storage::{
            MetadataStorePort, TrayIcon, TrayMenuItem, TrayMenuItemKind, TrayPort, TrayStatus,
        };

        use crate::adapters::{RuntimeAdapterProfile, FIXTURE_DEVICE};
        use crate::run_loop::{
            cleanup_disconnected_client, dashboard_store, ActiveSession, Live, RecordingStatusTx,
            RunLoopConfig,
        };

        /// Records every render instead of drawing it. The production tray's
        /// StatusNotifier host is a GUI boundary (a real icon needs a session
        /// bus plus an SNI watcher — see `KsniTray`), so the cleanup contract
        /// is pinned here at the [`TrayPort`] seam; the daemon-survives-the-
        /// disconnect half lives in the `daemon_run_lifecycle` e2e, which runs
        /// headless with the tray disabled and therefore cannot observe renders.
        #[derive(Default)]
        struct RecordingTray {
            icons: Vec<TrayIcon>,
            tooltips: Vec<String>,
            menus: Vec<Vec<TrayMenuItem>>,
        }

        impl TrayPort for RecordingTray {
            type Error = KsniTrayError;

            fn set_icon(&mut self, icon: TrayIcon) -> Result<(), Self::Error> {
                self.icons.push(icon);
                Ok(())
            }

            fn set_tooltip(&mut self, tooltip: &str) -> Result<(), Self::Error> {
                self.tooltips.push(tooltip.to_owned());
                Ok(())
            }

            fn set_menu(&mut self, items: Vec<TrayMenuItem>) -> Result<(), Self::Error> {
                self.menus.push(items);
                Ok(())
            }

            fn set_status(&mut self, _status: TrayStatus) -> Result<(), Self::Error> {
                Ok(())
            }
        }

        pub(super) fn test_config() -> RunLoopConfig {
            RunLoopConfig {
                socket_path: PathBuf::new(),
                data_dir: PathBuf::new(),
                database_path: PathBuf::new(),
                audio_root: PathBuf::new(),
                decoded_cache_root: PathBuf::new(),
                user_id: "test-user".to_owned(),
                shutdown_after_client: true,
                adapter_profile: RuntimeAdapterProfile {
                    audio_input_device: FIXTURE_DEVICE.to_owned(),
                    vad_engine: "energy".to_owned(),
                    asr_engine: "fixture".to_owned(),
                    whisper_model_path: PathBuf::new(),
                    asr_use_gpu: false,
                    asr_language: "en".to_owned(),
                    asr_threads: 1,
                },
                history_config: HistoryConfig::default(),
                translation_config: TranslationConfig::default(),
                vad_config: VadConfig::default(),
                notify_command: String::new(),
            }
        }

        #[test]
        fn every_helper_launcher_reports_through_the_configured_notify_command() {
            // This is the ONLY line joining the supervision code to a real
            // desktop. Nothing downstream of it can tell a misdirected notifier
            // from a working one: replacing these arguments with "" disables
            // every helper-failure alert in production, and `notify_user`
            // treats an empty command as "notifications off", so the whole
            // feature goes quiet with the suite still green.
            let mut config = test_config();
            config.notify_command = "/opt/custom-notifier".to_owned();

            let helpers = crate::run_loop::HelperLaunchers::discover(&config);

            assert_eq!(
                helpers.retention_dialog.notify_command(),
                "/opt/custom-notifier"
            );
            assert_eq!(
                helpers.settings_window.notify_command(),
                "/opt/custom-notifier"
            );
            assert_eq!(helpers.sync_panel.notify_command(), "/opt/custom-notifier");
        }

        #[test]
        fn the_dashboard_store_hands_over_the_history_key_only_when_encryption_is_on() {
            // The dashboard's sync ingest writes ime_text_history rows into the
            // daemon's database; when the daemon encrypts at rest, it must hand
            // the SAME key file it ciphers with (history.key beside the db —
            // one derivation, shared with build_history_cipher) — and when it
            // doesn't, it must hand nothing, else the dashboard would encrypt
            // rows the daemon reads plaintext.
            let mut config = test_config();
            config.database_path = PathBuf::from("/data/root/db/idiolect.sqlite");

            config.history_config.encrypt_at_rest = true;
            assert_eq!(
                dashboard_store(&config).history_key,
                Some(PathBuf::from("/data/root/db/history.key")),
            );

            config.history_config.encrypt_at_rest = false;
            assert_eq!(dashboard_store(&config).history_key, None);
        }

        /// Whether any (possibly nested) item is a history entry — the menu
        /// renders one per `recent_history` row, id `history:<row id>`.
        fn contains_history_item(items: &[TrayMenuItem]) -> bool {
            items.iter().any(|item| {
                item.id.starts_with("history:")
                    || matches!(
                        &item.kind,
                        TrayMenuItemKind::Standard { submenu: Some(sub) }
                            if contains_history_item(sub)
                    )
            })
        }

        #[test]
        fn a_disconnect_cleanup_republishes_idle_to_the_tray() {
            // A client that vanishes mid-take dies AFTER `start_live_capture`
            // rendered the tray as Recording; nothing else repaints it before
            // the next take, so the icon would lie ("Recording" with the mic
            // already released) until some unrelated state change. The unwind
            // path itself must re-publish Idle — via the tray only, never the
            // dead stream.
            let mut tray = RecordingTray::default();
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            store.migrate().expect("migrate");
            let config = test_config();
            // A real uncommitted session: the cleanup's cancel inserts the ONLY
            // history row, so the re-rendered menu containing a history entry
            // proves the render ran AFTER the cancel — an earlier render could
            // not show a row that did not exist yet.
            let session_id = store
                .create_session(Some("mid-take words"))
                .expect("create session");
            let mut live = Live {
                active_session: Some(ActiveSession {
                    session_id,
                    current_text: "mid-take words".to_owned(),
                    finalized: false,
                    tail_text: None,
                }),
                live_capture: None,
                live_stream: None,
                // As a start leaves it: `true` was the last published value.
                status_tx: RecordingStatusTx {
                    feature: true,
                    activity_feature: false,
                    last: ActivityPhase::Recording,
                },
                wants_reconcile: false,
            };

            cleanup_disconnected_client(&mut tray, &mut store, &config, &mut live)
                .expect("cleanup succeeds");

            assert_eq!(
                tray.icons.last(),
                Some(&TrayIcon::Idle),
                "the unwind must repaint the tray icon as idle"
            );
            assert_eq!(
                tray.tooltips.last().map(String::as_str),
                Some("Idiolect — Ready"),
                "the tooltip must drop the recording banner"
            );
            assert!(
                tray.menus
                    .last()
                    .is_some_and(|menu| contains_history_item(menu)),
                "the re-rendered menu must already show the just-cancelled take: \
                 the repaint has to happen after the session cancel"
            );
        }
    }

    /// Pinned at the unit level only, and deliberately.
    ///
    /// The defect is a store read failing for ONE call — the tray menu's
    /// `recent_history` — while the surrounding take keeps working. Integration
    /// and e2e both drive a real `SqliteMetadataStore`, which the daemon holds
    /// concretely from `run` all the way down, so the only way to break that one
    /// read is to break the database for every other statement too: the finalize
    /// then fails on its own account and the run proves nothing about this
    /// ordering. A fault-injecting store would need `MetadataStorePort`
    /// threaded through the whole stop path, which is a refactor of the daemon,
    /// not a test. The seam used here is the same `TrayPort`/store boundary the
    /// disconnect-cleanup contract is already pinned at.
    mod presentation_never_aborts_a_take {
        use idiolect_adapter_ksni::KsniTrayError;
        use idiolect_adapter_sqlite::SqliteMetadataStore;
        use idiolect_ipc::messages::ActivityPhase;
        use idiolect_ports::storage::{
            MetadataStorePort, TrayIcon, TrayMenuItem, TrayPort, TrayStatus,
        };

        use super::disconnect_cleanup::test_config;
        use crate::run_loop::{cleanup_disconnected_client, Live, RecordingStatusTx};

        /// Renders nothing and fails nothing, so the only failure in play comes
        /// from the STORE — the tray menu's `recent_history` read.
        struct SilentTray;

        impl TrayPort for SilentTray {
            type Error = KsniTrayError;

            fn set_icon(&mut self, _icon: TrayIcon) -> Result<(), Self::Error> {
                Ok(())
            }

            fn set_tooltip(&mut self, _tooltip: &str) -> Result<(), Self::Error> {
                Ok(())
            }

            fn set_menu(&mut self, _items: Vec<TrayMenuItem>) -> Result<(), Self::Error> {
                Ok(())
            }

            fn set_status(&mut self, _status: TrayStatus) -> Result<(), Self::Error> {
                Ok(())
            }
        }

        #[test]
        fn a_failed_tray_repaint_never_takes_the_daemon_down_with_the_client() {
            // The disconnect cleanup exists so one client going away cannot kill
            // the daemon ("crashing instead would let any engine restart take
            // the whole daemon down"). Its last act is a tray repaint, which
            // reads history out of SQLite — so a database that is briefly busy
            // hands the accept loop a non-disconnect error, and that IS fatal.
            // Every client disconnect would then be a coin flip on the daemon
            // surviving: precisely what this path was written to prevent.
            let mut tray = SilentTray;
            // Unmigrated, and with no session to cancel the repaint is the only
            // thing here that touches the store.
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            let config = test_config();
            let mut live = Live {
                active_session: None,
                live_capture: None,
                live_stream: None,
                status_tx: RecordingStatusTx::new(true, true),
                wants_reconcile: false,
            };

            cleanup_disconnected_client(&mut tray, &mut store, &config, &mut live)
                .expect("a failed repaint must not turn a disconnect into a daemon exit");
        }

        #[test]
        fn a_failed_tray_repaint_never_aborts_the_take_that_is_still_in_memory() {
            // The decode announcement runs at the one moment a take exists ONLY
            // in memory: `finish_capture` has drained the microphone, and
            // nothing has been folded or finalized yet. `set_phase` repaints the
            // tray, whose menu renders `recent_history` — a SQLite read that can
            // fail transiently while another process (trainer, sync-server,
            // trainerctl) writes the database. Propagating that error skips the
            // fold/finalize, and because the accept loop treats a non-disconnect
            // error as fatal it also stops the daemon: the audio the user just
            // spoke is gone. A repaint is presentation and must never be able to
            // do that.
            let mut tray = SilentTray;
            // Unmigrated on purpose: `recent_history` hits a missing table,
            // which is the same Err path a briefly unreadable database takes.
            let store = SqliteMetadataStore::open_in_memory().expect("store");
            assert!(
                store.recent_history(5).is_err(),
                "fixture must actually fail, or this test proves nothing",
            );
            let config = test_config();
            let (mut daemon_side, client_side) =
                std::os::unix::net::UnixStream::pair().expect("socketpair");
            let mut tx = RecordingStatusTx::new(true, true);

            tx.set_phase(
                &mut daemon_side,
                &mut tray,
                &store,
                &config,
                ActivityPhase::Transcribing,
            )
            .expect("a failed tray repaint must not fail the take");

            // And the pushes still reach the client: degrading the presentation
            // must not also silence the state the engine mirrors. Authoritative
            // `RecordingStatus` first, then the phase — the documented order.
            let mut reader = std::io::BufReader::new(client_side);
            let mut recording = String::new();
            let mut activity = String::new();
            std::io::BufRead::read_line(&mut reader, &mut recording).expect("read status");
            std::io::BufRead::read_line(&mut reader, &mut activity).expect("read phase");
            assert!(
                recording.contains(r#""recording":true"#),
                "the take must still be published as in flight: {recording}",
            );
            assert!(
                activity.contains("transcribing"),
                "the phase must still be published: {activity}",
            );
        }
    }

    mod capture_persist {
        use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
        use idiolect_common::digest::audio_sha256_hex;
        use idiolect_ports::audio::EncodedAudio;

        use crate::run_loop::persist_session;

        #[test]
        fn persisting_a_capture_records_the_audio_digest() {
            // The production gap S0a closes: a real capture must populate
            // `utterances.audio_sha256` (content digest of the encoded payload),
            // because the trainer's manifest builder rejects an empty digest.
            let tmp = tempfile::tempdir().expect("tempdir");
            let audio_store =
                FileAudioStore::new(tmp.path().join("audio"), tmp.path().join("decoded"));
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            store.migrate().expect("migrate");

            let payload = b"IDOPUS1 fake encoded opus payload".to_vec();
            let encoded = EncodedAudio {
                codec_name: "opus".to_owned(),
                sample_rate_hz: 16_000,
                channels: 1,
                payload: payload.clone(),
            };

            let session_id = persist_session(
                &mut store,
                &audio_store,
                "default",
                &encoded,
                "restart traffic",
            )
            .expect("persist should succeed");

            let link = store
                .session_utterance_link_for_test(session_id)
                .expect("link should query")
                .expect("link should exist");

            // The stored audio file landed...
            assert!(
                audio_store
                    .source_audio_exists_for_test(&idiolect_ports::storage::AudioObjectRef {
                        object_key: format!("audio/1970/01/01/default/{}.ogg", link.utterance_id),
                        codec_name: "opus".to_owned(),
                        sample_rate_hz: 16_000,
                        channels: 1,
                    })
                    .expect("exists query"),
                "capture must write the source audio",
            );
            // ...and the utterance row carries the digest of exactly those bytes.
            assert_eq!(
                store
                    .audio_digest_for_test(&link.utterance_id)
                    .expect("digest should query"),
                Some(audio_sha256_hex(&payload)),
            );
        }
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

    mod edit_via_ime {
        use std::io::{BufRead, BufReader, ErrorKind, Read};
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        use idiolect_adapter_sqlite::SqliteMetadataStore;
        use idiolect_ipc::messages::IpcMessage;
        use idiolect_ports::storage::MetadataStorePort;

        use crate::run_loop::{apply_history_edit, edit_entry_via_ime};

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
        fn edit_sends_the_entry_text_as_edit_history_to_the_engine() {
            // The daemon must forward an `EditHistory` (not `InsertText`) down the
            // engine socket so the engine can seed the review dialog with the
            // stored text. The id must round-trip so the engine's response carries
            // the correct entry id back.
            let (store, id) = store_with_entry("restart traefik");
            let (engine_side, mut daemon_side) = UnixStream::pair().expect("socketpair");

            edit_entry_via_ime(&mut daemon_side, &store, id).expect("edit");
            drop(daemon_side);

            let mut reader = BufReader::new(engine_side);
            let mut line = String::new();
            reader.read_line(&mut line).expect("read");
            assert!(
                !line.is_empty(),
                "edit must send a message, not type nothing"
            );
            match idiolect_ipc::framing::decode_json_line(&line).expect("decode") {
                IpcMessage::EditHistory(edit) => {
                    assert_eq!(edit.id, id);
                    assert_eq!(edit.text, "restart traefik");
                }
                other => panic!("expected EditHistory, got {other:?}"),
            }
        }

        #[test]
        fn edit_of_a_missing_entry_sends_nothing() {
            let (store, id) = store_with_entry("present");
            let (engine_side, mut daemon_side) = UnixStream::pair().expect("socketpair");

            edit_entry_via_ime(&mut daemon_side, &store, id + 999).expect("edit");

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

        #[test]
        fn apply_history_edit_amends_the_stored_entry() {
            // A confirmed review: the stored text must be updated to the corrected
            // form and the function returns Ok(true).
            let (mut store, id) = store_with_entry("restart traffic");

            let result = apply_history_edit(&mut store, id, "restart Traefik").expect("amend");
            assert!(result, "should return true when entry found");

            // The corrected text must be persisted so the tray lists the fix and a
            // re-edit starts from it — not left stale on the original transcript.
            let entry = store
                .get_history_entry(id)
                .expect("lookup")
                .expect("exists");
            assert_eq!(entry.text, "restart Traefik");
        }

        #[test]
        fn apply_history_edit_returns_false_for_missing_id() {
            let (mut store, id) = store_with_entry("present");

            let result = apply_history_edit(&mut store, id + 999, "corrected").expect("no error");
            assert!(!result, "should return false when entry not found");
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

        // The daemon's real resampler + VAD glue feeding the shared auto-stop
        // clock: real silence after the take's first speech crosses the threshold
        // (the "I paused for ages and nothing happened" fix), while pre-speech
        // silence (thinking time) never does. The threshold arithmetic itself is
        // unit-tested on the orchestration; this proves the wiring. (The per-take
        // dedup and the threshold rules live in
        // `idiolect_application::use_cases::streaming::take_tests`.)
        #[test]
        fn real_silence_after_speech_flags_auto_stop() {
            let mut state = LiveStreamState::new(&VadConfig {
                auto_stop_silence_ms: 2_000,
                ..VadConfig::default()
            });
            let silence_second = idiolect_ports::audio::AudioSegment {
                sample_rate_hz: 16_000,
                channels: 1,
                duration_ms: 1_000,
                samples_f32_mono: vec![0.0; 16_000],
            };

            // Three seconds of pre-speech silence: no auto-stop.
            for _ in 0..3 {
                assert!(state.ingest(&silence_second).is_empty());
            }
            assert!(
                !state.auto_stop_due(),
                "pre-speech silence never stops the take"
            );

            // Speak, then go quiet past the 2 s threshold.
            state.ingest(&speech_pause_speech_fixture_16khz_mono());
            state.ingest(&silence_second);
            state.ingest(&silence_second);
            assert!(state.auto_stop_due(), "2s threshold crossed after speech");
        }
    }

    mod preview_typing_setting {
        use idiolect_adapter_sqlite::SqliteMetadataStore;
        use idiolect_ports::storage::MetadataStorePort;

        use crate::run_loop::{preview_typing_enabled, toggle_preview_typing};

        fn store() -> SqliteMetadataStore {
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            store.migrate().expect("migrate");
            store
        }

        #[test]
        fn defaults_on_and_reads_the_stored_value() {
            let mut store = store();
            // Absent ⇒ ON, so the live-preview behaviour is on for everyone with no
            // migration needed.
            assert!(preview_typing_enabled(&store));
            store
                .set_tray_setting("preview_typing", "false")
                .expect("set");
            assert!(!preview_typing_enabled(&store));
            store
                .set_tray_setting("preview_typing", "true")
                .expect("set");
            assert!(preview_typing_enabled(&store));
        }

        #[test]
        fn toggle_flips_and_persists() {
            let mut store = store();
            // ON by default → first toggle turns it OFF, second back ON.
            toggle_preview_typing(&mut store).expect("toggle");
            assert!(!preview_typing_enabled(&store));
            toggle_preview_typing(&mut store).expect("toggle");
            assert!(preview_typing_enabled(&store));
        }
    }

    mod dictation_timing_settings {
        use idiolect_adapter_sqlite::SqliteMetadataStore;
        use idiolect_common::config::VadConfig;
        use idiolect_ports::storage::MetadataStorePort;

        use crate::run_loop::{apply_dictation_tray_action, effective_vad_config};

        fn store() -> SqliteMetadataStore {
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            store.migrate().expect("migrate");
            store
        }

        // The tray click is a GUI boundary; this is the logic it drives: index →
        // milliseconds, persisted as overrides that layer over the config file.
        #[test]
        fn tray_picks_persist_and_layer_over_config_defaults() {
            let mut store = store();
            let defaults = VadConfig::default();

            assert_eq!(effective_vad_config(&store, &defaults), defaults);

            // "Send a phrase after a pause of" → 0.4 s; "Ignore noises" → 0.4 s;
            // "Force-split" → 60 s; "Stop listening" → 10 s.
            assert!(apply_dictation_tray_action(&mut store, "settings:pause:0").expect("pause"));
            assert!(
                apply_dictation_tray_action(&mut store, "settings:min_speech:2").expect("blip")
            );
            assert!(
                apply_dictation_tray_action(&mut store, "settings:max_phrase:2").expect("phrase")
            );
            assert!(apply_dictation_tray_action(&mut store, "settings:auto_stop:2").expect("stop"));

            let effective = effective_vad_config(&store, &defaults);
            assert_eq!(effective.post_roll_ms, 400);
            assert_eq!(effective.min_speech_ms, 400);
            assert_eq!(effective.max_utterance_ms, 60_000);
            assert_eq!(effective.auto_stop_silence_ms, 10_000);

            // Back to "Never" via index 0.
            assert!(
                apply_dictation_tray_action(&mut store, "settings:auto_stop:0").expect("never")
            );
            assert_eq!(
                effective_vad_config(&store, &defaults).auto_stop_silence_ms,
                0
            );
        }

        #[test]
        fn auto_stop_below_the_pause_is_lifted_to_the_pause() {
            // A slow pause (2 s) combined with a 5 s auto-stop is fine, but if
            // overrides ever put auto-stop below the pause, the take could end
            // before one snippet completes — the effective value lifts to the
            // pause threshold instead of misbehaving.
            let mut store = store();
            let defaults = VadConfig::default();
            store
                .set_tray_setting("vad_post_roll_ms", "2000")
                .expect("set");
            store
                .set_tray_setting("vad_auto_stop_silence_ms", "1000")
                .expect("set");

            let effective = effective_vad_config(&store, &defaults);
            assert_eq!(effective.post_roll_ms, 2_000);
            assert_eq!(effective.auto_stop_silence_ms, 2_000, "lifted to the pause");
        }

        #[test]
        fn corrupt_overrides_and_foreign_actions_are_safe() {
            let mut store = store();
            let defaults = VadConfig::default();
            store
                .set_tray_setting("vad_post_roll_ms", "banana")
                .expect("set");

            assert_eq!(
                effective_vad_config(&store, &defaults).post_roll_ms,
                defaults.post_roll_ms,
                "unparseable override falls back to the config default"
            );

            // Out-of-range index is consumed but changes nothing.
            assert!(apply_dictation_tray_action(&mut store, "settings:pause:99").expect("oob"));
            assert_eq!(
                effective_vad_config(&store, &defaults).post_roll_ms,
                defaults.post_roll_ms
            );
            // Non-timing actions are left for the other handlers.
            assert!(!apply_dictation_tray_action(&mut store, "review_mode").expect("foreign"));
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

        // `dashboard_store` hands the history key to the dashboard subprocess
        // based on the config-file value alone; if a tray override could flip
        // encryption at runtime, the daemon and the dashboard would cipher the
        // SAME database differently. Pin that the override layer never touches
        // it.
        #[test]
        fn encrypt_at_rest_is_config_file_only_never_a_tray_override() {
            use idiolect_ports::storage::MetadataStorePort;
            let mut store = SqliteMetadataStore::open_in_memory().expect("store");
            store.migrate().expect("migrate");
            store
                .set_tray_setting("encrypt_at_rest", "true")
                .expect("stale/hand-edited setting");

            let defaults = HistoryConfig::default();
            assert!(!effective_history_config(&store, &defaults).encrypt_at_rest);
        }
    }
}
