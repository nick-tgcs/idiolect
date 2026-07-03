//! The single UniFFI facade the Android app loads — the in-process replacement
//! for the desktop's Unix-socket daemon.
//!
//! The desktop runs a separate-process daemon and talks to it over a Unix socket
//! with the [`idiolect_ipc`] message vocabulary. On Android there is no daemon:
//! the IME service loads this `.so` and calls the same logic **in process**. So
//! the socket's command/event pairs collapse into:
//!
//! * **Kotlin → Rust**: the methods on [`IdiolectCore`] (`toggle`,
//!   `push_pcm_frame`, `cancel`, `report_correction`, `load_model`, history ops).
//! * **Rust → Kotlin**: the [`IdiolectInputMethod`] callback (the analog of the
//!   `RecordingStatus`/`PreeditUpdate`/`InsertText`/`EditHistory` server pushes).
//!
//! The brain and the **streaming take** are reused unchanged. A live take is the
//! shared `idiolect_application::use_cases::streaming::StreamingTake` orchestration
//! M2 lifted out of the daemon: `push_pcm_frame` feeds [`StreamingTake::ingest`]
//! (16 kHz mono direct from `AudioRecord`, labelled by the on-device WebRTC VAD);
//! each pause-completed snippet is decoded by an on-device Whisper
//! [`TakeTranscriber`] and folded via `fold_snippet`, pushing a live partial
//! preedit; `toggle`-to-stop flushes the tail, decodes the WHOLE recording once
//! (the authoritative text — the *streaming-drops-words* fix), and persists the
//! take as ONE session + Opus recording before committing it into the field. This
//! is the same finalize-creates-the-session shape the daemon uses, so both
//! front-ends are governed by one orchestration.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use idiolect_adapter_crypto::ChaCha20Poly1305Cipher;
use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore, SqliteStorageError};
use idiolect_adapter_vad::VadAdapter;
use idiolect_adapter_whisper::{WhisperAsr, WhisperOptions};
use idiolect_application::use_cases::dictation::{DictationUseCase, DictationUseCaseError};
use idiolect_application::use_cases::streaming::{
    StreamObserver, StreamingConfig, StreamingTake, TakeOutcome, TakeTranscriber, TranscribeFailure,
};
use idiolect_common::config::{PathProvider, RootedPaths, VadConfig};
use idiolect_common::ids::{utterance_id_for_session, ImeSessionId, UserId};
use idiolect_ports::asr::AsrPort;
use idiolect_ports::audio::AudioSegment;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::input_method::InputMethodPort;
use idiolect_ports::storage::{AudioStorePort, HistoryEntry, HistoryState, MetadataStorePort};
use idiolect_sync::{decode_batch, encode_batch};
use idiolect_sync_client::{build_batch, confirm_shipped, SyncClientError};

uniffi::setup_scaffolding!();

/// The fixed live-pipeline geometry: 16 kHz mono. `AudioRecord` is configured to
/// capture at exactly this rate, so — unlike the desktop — there is no resampler.
const STREAM_SAMPLE_RATE_HZ: u32 = 16_000;
/// Full-scale value of a signed-16-bit sample, for the i16 → f32 conversion the
/// brain works in.
const I16_FULL_SCALE: f32 = 32_768.0;

/// Every fallible operation the Android app can trigger maps onto this error so
/// the Kotlin side gets a typed failure rather than a panic across the FFI.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    // The error variants' payload field is `detail`, not `message`: UniFFI generates
    // each variant as a `kotlin.Exception` subclass, and a field named `message`
    // collides with `Throwable.message` (the generated `override val message` then
    // fails to compile). `detail` carries the same string.
    #[error("storage error: {detail}")]
    Storage { detail: String },
    #[error("no active dictation take")]
    NoActiveTake,
    #[error("history entry {id} not found")]
    HistoryEntryNotFound { id: i64 },
    #[error("io error: {detail}")]
    Io { detail: String },
    #[error("history key must be 32 bytes, got {len}")]
    InvalidHistoryKey { len: u32 },
    #[error("model integrity check failed: expected {expected}, got {actual}")]
    ModelIntegrity { expected: String, actual: String },
}

impl From<SqliteStorageError> for FfiError {
    fn from(error: SqliteStorageError) -> Self {
        Self::Storage {
            detail: error.to_string(),
        }
    }
}

impl From<SyncClientError> for FfiError {
    fn from(error: SyncClientError) -> Self {
        match error {
            SyncClientError::Storage(error) => Self::Storage {
                detail: error.to_string(),
            },
            SyncClientError::Audio(error) => Self::Io {
                detail: error.to_string(),
            },
        }
    }
}

impl From<DictationUseCaseError<Infallible, SqliteStorageError>> for FfiError {
    fn from(error: DictationUseCaseError<Infallible, SqliteStorageError>) -> Self {
        match error {
            // The callback front-end is infallible (see `CallbackInput`), so this
            // arm is unreachable; matching the empty type proves it to the compiler.
            DictationUseCaseError::Input(infallible) => match infallible {},
            DictationUseCaseError::Storage(error) => error.into(),
        }
    }
}

/// Rust → Kotlin push channel: the front-end (an Android `InputMethodService`
/// owning the `InputConnection`) implements this. It mirrors the desktop
/// `InputMethodPort` plus the daemon's authoritative recording-state and history
/// pushes. The daemon owns the microphone and is the single source of truth for
/// recording state; the Android UI likewise waits for [`Self::recording_status`]
/// rather than toggling its mic indicator optimistically.
///
/// **Re-entrancy contract (must hold):** these callbacks are invoked synchronously
/// on the calling thread **while [`IdiolectCore`]'s lock is held**, so an
/// implementation MUST NOT call back into the same `IdiolectCore` from within a
/// callback (the lock is not reentrant — it would deadlock). Apply the edits to the
/// `InputConnection` and post any UI work to the main thread; never invoke
/// `toggle`/`push_pcm_frame`/`is_recording`/etc. from inside a callback. (A later
/// increment can lift callbacks out of the lock; until then this contract stands.)
#[uniffi::export(callback_interface)]
pub trait IdiolectInputMethod: Send + Sync {
    /// Authoritative recording state, edge-triggered (the single source of truth).
    fn recording_status(&self, recording: bool);
    /// Begin a live preedit for the take (`setComposingText`).
    fn show_preedit(&self, text: String);
    /// Update the live preedit in place (`setComposingText`).
    fn update_preedit(&self, text: String);
    /// Finalise the take into the field (`commitText`).
    fn commit_text(&self, text: String);
    /// Clear the live preedit without committing (`finishComposingText`).
    fn cancel_preedit(&self);
    /// Type stored text directly at the cursor (history re-insert).
    fn insert_text(&self, text: String);
    /// Open the review dialog seeded with a stored history entry's text.
    fn edit_history(&self, id: i64, text: String);
    /// A take failed to decode (no model loaded, or the engine errored): tell the
    /// user, at most once per take — the Android analog of the desktop's failure
    /// notification.
    fn dictation_error(&self, message: String);
}

/// Adapts the Kotlin callback to the `InputMethodPort` the brain drives. The
/// per-session id is dropped: on Android there is exactly one active
/// `InputConnection`, so the session is implicit. Forwarding to a callback cannot
/// fail back into Rust, hence `Error = Infallible`.
struct CallbackInput {
    callback: Arc<dyn IdiolectInputMethod>,
}

impl InputMethodPort for CallbackInput {
    type Error = Infallible;

    fn show_preedit(&mut self, _session_id: ImeSessionId, text: &str) -> Result<(), Infallible> {
        self.callback.show_preedit(text.to_owned());
        Ok(())
    }
    fn update_preedit(&mut self, _session_id: ImeSessionId, text: &str) -> Result<(), Infallible> {
        self.callback.update_preedit(text.to_owned());
        Ok(())
    }
    fn commit_text(&mut self, _session_id: ImeSessionId, text: &str) -> Result<(), Infallible> {
        self.callback.commit_text(text.to_owned());
        Ok(())
    }
    fn cancel_preedit(&mut self, _session_id: ImeSessionId) -> Result<(), Infallible> {
        self.callback.cancel_preedit();
        Ok(())
    }
}

/// Routes a live take's events to the Kotlin callback. Holds the take's
/// accumulating preedit because Android's `setComposingText` **replaces** the
/// whole composing region — so each snippet must push the full text so far, not
/// just its new chunk. The first snippet of a take begins the preedit
/// (`show_preedit`); later ones update it in place.
struct CallbackObserver<'a> {
    callback: &'a dyn IdiolectInputMethod,
    preedit: &'a mut String,
    started: &'a mut bool,
}

impl StreamObserver for CallbackObserver<'_> {
    // Callback forwarding cannot fail back into Rust.
    type Error = Infallible;

    fn snippet_committed(&mut self, chunk: &str) -> Result<(), Infallible> {
        self.preedit.push_str(chunk);
        if *self.started {
            self.callback.update_preedit(self.preedit.clone());
        } else {
            self.callback.show_preedit(self.preedit.clone());
            *self.started = true;
        }
        Ok(())
    }

    fn snippet_dropped(&mut self, _decoded: &str) -> Result<(), Infallible> {
        // Noise-only or empty: its audio is kept for the stop-time decode, but
        // there is nothing to type.
        Ok(())
    }

    fn transcribe_failed(&mut self, _code: &str, message: &str) -> Result<(), Infallible> {
        // The orchestration already de-duplicates this to once per take per cause.
        self.callback.dictation_error(message.to_owned());
        Ok(())
    }

    fn finalize_progress(&mut self, full_text: &str) -> Result<(), Infallible> {
        // The stop-time re-decode advanced by one ≤30 s chunk: replace the whole
        // composing region with the take as it now stands (decoded chunks +
        // still-preview tail), so a long take firms up in place instead of one big
        // swap. `setComposingText` replaces the region, so pushing the full text is
        // exactly right. Track it as the take's preedit so the eventual commit lines up.
        if *self.started && self.preedit == full_text {
            // A chunk that re-decoded to exactly its preview: nothing to redraw.
            return Ok(());
        }
        self.preedit.clear();
        self.preedit.push_str(full_text);
        if *self.started {
            self.callback.update_preedit(full_text.to_owned());
        } else {
            self.callback.show_preedit(full_text.to_owned());
            *self.started = true;
        }
        Ok(())
    }
}

/// One row of dictation history, as the Kotlin history screen renders it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct HistoryItem {
    pub id: i64,
    pub text: String,
    /// `true` for a committed take, `false` for a cancelled one.
    pub committed: bool,
}

impl From<HistoryEntry> for HistoryItem {
    fn from(entry: HistoryEntry) -> Self {
        Self {
            id: entry.id,
            text: entry.text,
            committed: matches!(entry.state, HistoryState::Committed),
        }
    }
}

/// The decode port until a real model is loaded: every snippet/finalize "fails"
/// with a stable code, so a take recorded before a model exists produces nothing
/// (and the user is told once) rather than silently appearing broken.
struct UnavailableTranscriber;

impl TakeTranscriber for UnavailableTranscriber {
    fn transcribe(&mut self, _samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure> {
        Err(TranscribeFailure {
            code: "asr-unavailable".to_owned(),
            message: "no speech model loaded — download a model in settings first".to_owned(),
        })
    }
}

/// Binds on-device Whisper to the take's decode port (CPU; the `cuda` feature is
/// off on Android). Mirrors the daemon's `DaemonTranscriber`: build a 16 kHz mono
/// segment and transcribe.
struct WhisperTakeTranscriber {
    asr: WhisperAsr,
}

impl TakeTranscriber for WhisperTakeTranscriber {
    fn transcribe(&mut self, samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure> {
        self.asr
            .transcribe(&segment_from_samples(samples_f32_mono))
            .map(|draft| draft.text)
            .map_err(|error| TranscribeFailure {
                code: "asr-error".to_owned(),
                message: error.to_string(),
            })
    }
}

/// A `Send` wrapper around the WebRTC VAD.
///
/// `webrtc_vad::Vad` holds a raw `*mut Fvad` and so is `!Send`, but the VAD is
/// reachable only through [`IdiolectCore`]'s mutex — never touched by two threads
/// at once — and the fvad handle is a plain heap object with no thread affinity.
struct SendVad(VadAdapter);

// SAFETY: `SendVad` is only ever accessed while holding the `IdiolectCore` mutex,
// so there is no concurrent access; the wrapped fvad handle has no thread-local
// state, so moving it between threads is sound.
unsafe impl Send for SendVad {}

/// Mutable state behind the [`IdiolectCore`] mutex.
/// Default cap on captured (not-yet-shipped) source audio: 1 GiB. The oldest
/// captures are evicted past this so a phone that never pairs doesn't grow without
/// bound. Overridable via [`IdiolectCore::set_audio_storage_cap_bytes`].
const DEFAULT_AUDIO_STORAGE_CAP_BYTES: u64 = 1_073_741_824;

struct Inner {
    /// Drives `create_session`/`commit`/`cancel` over the real store and, through
    /// its [`CallbackInput`], the field-typing callbacks.
    dictation: DictationUseCase<CallbackInput, SqliteMetadataStore>,
    /// The same callback the `dictation` holds — for the live-preedit / error
    /// pushes that are not on `InputMethodPort`.
    callback: Arc<dyn IdiolectInputMethod>,
    /// Where the take's single Opus recording is written (the training-pair audio).
    audio_store: FileAudioStore,
    codec: OpusCodec,
    user_id: String,
    /// The decode engine the take folds snippets through; an
    /// [`UnavailableTranscriber`] until [`IdiolectCore::load_model`] swaps in
    /// Whisper.
    transcriber: Box<dyn TakeTranscriber + Send>,
    /// The per-frame speech detector, reset per take (its noise estimate must not
    /// bleed across takes).
    vad: SendVad,
    streaming_config: StreamingConfig,
    /// Authoritative recording state — the single source of truth (§ daemon).
    recording: bool,
    /// Whether the live take is in continuous mode: each phrase (between pauses) is
    /// committed into the field as the speaker pauses and the mic stays open, vs the
    /// one-shot take that commits the whole recording once at stop. Set by
    /// [`IdiolectCore::start_continuous`], cleared when recording stops.
    continuous: bool,
    /// The live take while recording.
    take: Option<StreamingTake>,
    /// The full preedit accumulated for the current take (for `setComposingText`).
    preedit: String,
    /// Whether the current take has begun its preedit (`show` vs `update`).
    preedit_started: bool,
    /// The most recently committed take, for an in-place [`IdiolectCore::report_correction`].
    last_commit: Option<(ImeSessionId, String)>,
    /// Cap on captured (not-yet-shipped) source audio; the oldest captures are
    /// evicted after each take to keep on-device audio under this many bytes.
    audio_storage_cap_bytes: u64,
}

/// The in-process core the Android app holds for the life of the IME service.
#[derive(uniffi::Object)]
pub struct IdiolectCore {
    inner: Mutex<Inner>,
}

#[uniffi::export]
impl IdiolectCore {
    /// Open (or create) the on-device store under `data_dir` (the app's private
    /// `filesDir`) and wire the brain to `callback`. No speech model is loaded yet
    /// — call [`Self::load_model`] before dictation can produce text.
    ///
    /// `history_key`, when present, must be exactly 32 bytes and enables at-rest
    /// encryption of the `ime_text_history` projection (`ChaCha20Poly1305`, reused
    /// unchanged from desktop). On Android the key is unwrapped from the
    /// hardware-backed Keystore; `null` leaves the projection plaintext (the prior
    /// behaviour, and what the host seam tests use).
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        history_key: Option<Vec<u8>>,
        callback: Box<dyn IdiolectInputMethod>,
    ) -> Result<Arc<Self>, FfiError> {
        let paths = RootedPaths::new(data_dir);
        let audio_dir = paths.audio_dir();
        let decoded_cache_dir = paths.data_dir().join("decoded-cache");
        for dir in [
            paths.data_dir(),
            audio_dir.clone(),
            decoded_cache_dir.clone(),
        ] {
            std::fs::create_dir_all(&dir).map_err(|error| FfiError::Io {
                detail: error.to_string(),
            })?;
        }
        let mut store = SqliteMetadataStore::open_path(paths.database_path())?;
        store.migrate()?;
        // Enable at-rest history encryption when a key is supplied (after migrate, as
        // the daemon does). A wrong-length key is a typed error, never a panic.
        let store = match history_key {
            Some(key) => {
                let key: [u8; 32] =
                    key.as_slice()
                        .try_into()
                        .map_err(|_| FfiError::InvalidHistoryKey {
                            len: key.len() as u32,
                        })?;
                store.with_history_cipher(Box::new(ChaCha20Poly1305Cipher::new(key)))
            }
            None => store,
        };
        let callback: Arc<dyn IdiolectInputMethod> = Arc::from(callback);
        let dictation = DictationUseCase::new(
            CallbackInput {
                callback: Arc::clone(&callback),
            },
            store,
        );
        Ok(Arc::new(Self {
            inner: Mutex::new(Inner {
                dictation,
                callback,
                audio_store: FileAudioStore::new(audio_dir, decoded_cache_dir),
                codec: OpusCodec::new(),
                user_id: UserId::default_user().as_str().to_owned(),
                transcriber: Box::new(UnavailableTranscriber),
                vad: SendVad(VadAdapter::new()),
                streaming_config: streaming_config_from(&VadConfig::default()),
                recording: false,
                continuous: false,
                take: None,
                preedit: String::new(),
                preedit_started: false,
                last_commit: None,
                audio_storage_cap_bytes: DEFAULT_AUDIO_STORAGE_CAP_BYTES,
            }),
        }))
    }

    /// Override the captured-audio storage cap (bytes). A future settings screen
    /// calls this; until then the default (1 GiB, [`DEFAULT_AUDIO_STORAGE_CAP_BYTES`])
    /// applies. Takes effect on the next finalized take.
    pub fn set_audio_storage_cap_bytes(&self, bytes: u64) {
        self.lock().audio_storage_cap_bytes = bytes;
    }

    /// Verify the model file's SHA-256 against `expected_sha256` (lowercase hex), then
    /// load it. This is the **production** entry point: it enforces the M5
    /// "per-file SHA-256 at every load" contract, so a corrupted or substituted model
    /// can never be loaded. (The unverified [`Self::load_model`] is for tests/fixtures.)
    pub fn load_model_verified(
        &self,
        model_path: String,
        expected_sha256: String,
    ) -> Result<(), FfiError> {
        let actual = idiolect_common::digest::file_sha256_hex(&model_path).map_err(|error| {
            FfiError::Io {
                detail: format!("hash model: {error}"),
            }
        })?;
        if !actual.eq_ignore_ascii_case(&expected_sha256) {
            return Err(FfiError::ModelIntegrity {
                expected: expected_sha256,
                actual,
            });
        }
        self.load_model(model_path)
    }

    /// Load the on-device speech model from `model_path` **without** integrity
    /// verification. Prefer [`Self::load_model_verified`] in production; this exists for
    /// the committed test fixture and the missing-file guard. Until a model loads, a
    /// take finalizes to nothing and the user is told a model is needed.
    pub fn load_model(&self, model_path: String) -> Result<(), FfiError> {
        // On-device decode tuned for latency: use the phone's cores (the default of 1
        // leaves a multi-core device mostly idle on the compute-bound matmuls) and
        // greedy decoding (`beam_size: 1`) — both are the dominant levers for how fast a
        // take finalizes. The desktop daemon keeps its own (beam-search) configuration.
        let options = WhisperOptions {
            n_threads: on_device_decode_threads(),
            beam_size: 1,
            ..WhisperOptions::default()
        };
        let asr = WhisperAsr::load(model_path, options).map_err(|error| FfiError::Io {
            detail: format!("load model: {error}"),
        })?;
        self.install_transcriber(Box::new(WhisperTakeTranscriber { asr }));
        Ok(())
    }

    /// One-tap mic key: the core alone decides start-vs-stop from its authoritative
    /// state (the daemon's `ToggleRecording` intent). Each edge pushes
    /// [`IdiolectInputMethod::recording_status`]; stopping decodes and commits the
    /// whole take.
    pub fn toggle(&self) -> Result<(), FfiError> {
        let mut inner = self.lock();
        if inner.recording {
            inner.stop_recording()
        } else {
            inner.continuous = false;
            inner.start_recording();
            Ok(())
        }
    }

    /// Begin a **continuous** take (the mic's double-tap gesture): each phrase is
    /// committed into the field as the speaker pauses and the mic stays open, until a
    /// plain [`Self::toggle`]/stop closes it (finalizing the last phrase). A stray call
    /// while already recording is ignored.
    pub fn start_continuous(&self) -> Result<(), FfiError> {
        let mut inner = self.lock();
        if inner.recording {
            return Ok(());
        }
        inner.continuous = true;
        inner.start_recording();
        Ok(())
    }

    /// Whether the live take is in continuous mode (drives the mic's "● Continuous" look).
    #[must_use]
    pub fn is_continuous(&self) -> bool {
        self.lock().continuous
    }

    /// Push one buffer of captured 16 kHz mono PCM (the mic FGS → JNI hot path)
    /// into the live take. Only valid while recording; a stray frame is rejected.
    /// A pause inside the buffer decodes a snippet and pushes a live partial.
    ///
    /// This holds the core lock across that (CPU Whisper) decode, so it can block
    /// for the snippet-decode duration and must be called off the audio thread;
    /// while it runs, `toggle`/`cancel` wait. Lifting the decode out of the lock is
    /// a tracked follow-up (M3 polish).
    pub fn push_pcm_frame(&self, frame: Vec<i16>) -> Result<(), FfiError> {
        self.lock().push_frame(&frame)
    }

    /// Discard the current recording without committing. The session is created
    /// only at finalize, so an aborted take leaves nothing persisted.
    pub fn cancel(&self) -> Result<(), FfiError> {
        let mut inner = self.lock();
        if !inner.recording {
            return Err(FfiError::NoActiveTake);
        }
        inner.take = None;
        inner.recording = false;
        inner.continuous = false;
        let had_preedit = inner.preedit_started;
        inner.preedit.clear();
        inner.preedit_started = false;
        if had_preedit {
            inner.callback.cancel_preedit();
        }
        inner.callback.recording_status(false);
        Ok(())
    }

    /// The user fixed the auto-committed text in the field: amend the just-committed
    /// take with the corrected form so a raw→corrected training pair is recorded and
    /// the history projection reflects it.
    ///
    /// `corrected_text` is the **whole** corrected take, read back from the field via
    /// `InputConnection` (the Android ground-truth-from-the-field model). This is a
    /// deliberate divergence from the desktop daemon, whose IBus correction window
    /// tracks only the take's final snippet and merges a *tail* via
    /// `merge_tail_correction`; Android's `InputConnection` exposes the entire field,
    /// so there is no tail to merge — the whole text replaces the committed text.
    pub fn report_correction(&self, corrected_text: String) -> Result<(), FfiError> {
        let mut inner = self.lock();
        let Some((session_id, committed)) = inner.last_commit.clone() else {
            return Ok(());
        };
        if corrected_text == committed {
            return Ok(());
        }
        inner
            .dictation
            .storage_mut()
            .amend_correction(session_id, &committed, &corrected_text)?;
        inner.last_commit = Some((session_id, corrected_text));
        Ok(())
    }

    /// Retroactively correct any past history entry (the review dialog result).
    /// Amends the stored record and rewrites its training pair.
    pub fn history_edited(&self, id: i64, corrected_text: String) -> Result<(), FfiError> {
        let mut inner = self.lock();
        let entry = inner
            .dictation
            .storage()
            .get_history_entry(id)?
            .ok_or(FfiError::HistoryEntryNotFound { id })?;
        inner.dictation.storage_mut().amend_correction(
            entry.session_id,
            &entry.text,
            &corrected_text,
        )?;
        // If the edited row is the just-committed take, keep `last_commit` in sync so
        // a later in-field `report_correction` amends from the new text rather than
        // stale text (mirrors the daemon's `HistoryEdited` keeping `current_text`
        // consistent).
        if matches!(&inner.last_commit, Some((session_id, _)) if *session_id == entry.session_id) {
            inner.last_commit = Some((entry.session_id, corrected_text));
        }
        Ok(())
    }

    /// Type a stored history entry's text directly at the cursor.
    pub fn reinsert_history(&self, id: i64) -> Result<(), FfiError> {
        let inner = self.lock();
        let entry = inner
            .dictation
            .storage()
            .get_history_entry(id)?
            .ok_or(FfiError::HistoryEntryNotFound { id })?;
        inner.callback.insert_text(entry.text);
        Ok(())
    }

    /// Ask the front-end to open the review dialog over a stored history entry.
    pub fn open_history_edit(&self, id: i64) -> Result<(), FfiError> {
        let inner = self.lock();
        let entry = inner
            .dictation
            .storage()
            .get_history_entry(id)?
            .ok_or(FfiError::HistoryEntryNotFound { id })?;
        inner.callback.edit_history(entry.id, entry.text);
        Ok(())
    }

    /// The most-recent dictation history (newest first), for the history screen.
    pub fn recent_history(&self, limit: u32) -> Result<Vec<HistoryItem>, FfiError> {
        let inner = self.lock();
        let entries = inner.dictation.storage().recent_history(limit)?;
        Ok(entries.into_iter().map(HistoryItem::from).collect())
    }

    /// The authoritative recording state, for the UI's initial paint (every later
    /// change arrives via [`IdiolectInputMethod::recording_status`]).
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.lock().recording
    }

    /// Export the local sync outbox — captured, not-yet-shipped raw→corrected
    /// learnings plus their content-addressed audio — as the on-the-wire sync
    /// container bytes the phone `POST`s to `/v1/sync`. Returns an **empty** vec when
    /// nothing is pending, so the WorkManager pump can cheaply skip a no-op sync
    /// rather than ship an encoded empty batch. `device_id` scopes server-side dedup;
    /// `batch_id` makes a re-POST idempotent (the caller mints a fresh one per
    /// attempt — re-export after a failed POST is safe, the server dedups by digest).
    pub fn export_sync_batch(
        &self,
        device_id: String,
        batch_id: String,
    ) -> Result<Vec<u8>, FfiError> {
        let inner = self.lock();
        let envelope = build_batch(
            inner.dictation.storage(),
            &inner.audio_store,
            &inner.user_id,
            &device_id,
            &batch_id,
        )?;
        if envelope.batch.learnings.is_empty() {
            return Ok(Vec::new());
        }
        encode_batch(&envelope).map_err(|error| FfiError::Io {
            detail: format!("encode sync batch: {error}"),
        })
    }

    /// After the PC acks a batch as durably stored, reclaim local storage for it:
    /// flip each shipped learning to `synced` and drop its on-device audio
    /// (delete-after-ACK). `batch` is the exact bytes [`Self::export_sync_batch`]
    /// returned and the phone POSTed; on a `200` every learning in it is stored
    /// (accepted or already-present), so the whole batch is safe to reclaim.
    pub fn confirm_synced(&self, batch: Vec<u8>) -> Result<(), FfiError> {
        let envelope = decode_batch(&batch).map_err(|error| FfiError::Io {
            detail: format!("decode sync batch: {error}"),
        })?;
        let mut inner = self.lock();
        let Inner {
            dictation,
            audio_store,
            ..
        } = &mut *inner;
        confirm_shipped(
            dictation.storage_mut(),
            audio_store,
            &envelope.batch.learnings,
        )?;
        Ok(())
    }
}

// Non-`#[uniffi::export]` seams: callable as ordinary Rust (so the host seam tests
// can inject a scripted decoder) but not surfaced to Kotlin.
impl IdiolectCore {
    /// Swap in the decode engine the live take folds snippets through.
    /// [`Self::load_model`] builds the real Whisper engine on top of this; the host
    /// seam tests inject a scripted one so the streaming wiring is deterministic
    /// without a model.
    pub fn install_transcriber(&self, transcriber: Box<dyn TakeTranscriber + Send>) {
        self.lock().transcriber = transcriber;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // Recover rather than brick the whole core if a previous call poisoned the
        // mutex. A Kotlin callback throwing across the FFI unwinds through the held
        // guard and poisons it; without this, every later call would panic at the
        // lock and dictation would be dead until the IME service is restarted. The
        // worst residual `Inner` state (e.g. `recording = true`, `take = None` after
        // a panic mid-finalize) self-heals on the next `toggle`.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Inner {
    /// Begin a fresh take: new state machine, a fresh VAD, and the authoritative
    /// recording push. No session row yet — it is created at finalize, seeded with
    /// the whole-recording transcript.
    fn start_recording(&mut self) {
        self.take = Some(StreamingTake::new(&self.streaming_config));
        self.vad = SendVad(VadAdapter::new());
        self.preedit.clear();
        self.preedit_started = false;
        self.recording = true;
        self.callback.recording_status(true);
    }

    /// Ingest one PCM buffer into the live take and fold any snippet a pause
    /// completed (each pushes a live partial preedit).
    fn push_frame(&mut self, frame: &[i16]) -> Result<(), FfiError> {
        if !self.recording {
            return Err(FfiError::NoActiveTake);
        }
        let samples: Vec<f32> = frame
            .iter()
            .map(|s| f32::from(*s) / I16_FULL_SCALE)
            .collect();

        // Ingest returns owned snippet audio, so the take borrow is released before the
        // per-snippet work below — which, in continuous mode, needs all of `self` to
        // commit a phrase after each pause.
        let snippets = {
            let Inner { take, vad, .. } = self;
            let Some(take) = take.as_mut() else {
                return Err(FfiError::NoActiveTake);
            };
            take.ingest(&samples, |frame| {
                vad.0.is_speech_frame(frame).unwrap_or(false)
            })
        };

        for snippet in snippets {
            self.fold_one(snippet)?;
            // Continuous: a pause completed this snippet — commit the phrase now and keep
            // the mic open. One-shot mode just keeps the preview; it commits once at stop.
            if self.continuous {
                let outcome = {
                    let Inner {
                        take,
                        transcriber,
                        callback,
                        preedit,
                        preedit_started,
                        ..
                    } = self;
                    let Some(take) = take.as_mut() else {
                        return Err(FfiError::NoActiveTake);
                    };
                    let mut observer = CallbackObserver {
                        callback: callback.as_ref(),
                        preedit,
                        started: preedit_started,
                    };
                    match take.finalize_phrase(transcriber, &mut observer) {
                        Ok(outcome) => outcome,
                        Err(infallible) => match infallible {},
                    }
                };
                self.commit_phrase(outcome)?;
            }
        }
        Ok(())
    }

    /// Fold one pause-completed snippet into the live take's preview preedit.
    fn fold_one(&mut self, snippet: Vec<f32>) -> Result<(), FfiError> {
        let Inner {
            take,
            transcriber,
            callback,
            preedit,
            preedit_started,
            ..
        } = self;
        let Some(take) = take.as_mut() else {
            return Err(FfiError::NoActiveTake);
        };
        let mut observer = CallbackObserver {
            callback: callback.as_ref(),
            preedit,
            started: preedit_started,
        };
        // The observer is infallible (callback forwarding cannot fail).
        match take.fold_snippet(transcriber, &mut observer, snippet) {
            Ok(()) => {}
            Err(infallible) => match infallible {},
        }
        Ok(())
    }

    /// Commit one finalized phrase (continuous mode) into the field and reset the live
    /// preview so the next phrase starts fresh — WITHOUT touching the recording state, so
    /// the mic stays open. Each phrase is its own session (its own idempotency key).
    fn commit_phrase(&mut self, outcome: TakeOutcome) -> Result<(), FfiError> {
        self.finalize_outcome(outcome)?;
        self.preedit.clear();
        self.preedit_started = false;
        Ok(())
    }

    /// Stop: recover the un-paused tail, decode the WHOLE take once (authoritative),
    /// persist it as one session + recording, and commit it into the field.
    fn stop_recording(&mut self) -> Result<(), FfiError> {
        let Some(mut take) = self.take.take() else {
            // Recording flag set with no take (shouldn't happen): just clear it.
            self.recording = false;
            self.callback.recording_status(false);
            return Ok(());
        };
        if let Some(tail) = take.flush() {
            let Inner {
                transcriber,
                callback,
                preedit,
                preedit_started,
                ..
            } = self;
            let mut observer = CallbackObserver {
                callback: callback.as_ref(),
                preedit,
                started: preedit_started,
            };
            match take.fold_snippet(transcriber, &mut observer, tail) {
                Ok(()) => {}
                Err(infallible) => match infallible {},
            }
        }
        // Re-decode the take chunk by chunk (the authoritative pass), firming up the
        // preedit in place as each ≤30 s chunk lands; a long take no longer collapses
        // to a truncated whole-recording decode.
        let outcome = {
            let Inner {
                transcriber,
                callback,
                preedit,
                preedit_started,
                ..
            } = self;
            let mut observer = CallbackObserver {
                callback: callback.as_ref(),
                preedit,
                started: preedit_started,
            };
            match take.finalize(transcriber, &mut observer) {
                Ok(outcome) => outcome,
                Err(infallible) => match infallible {},
            }
        };
        let result = self.finalize_outcome(outcome);
        self.recording = false;
        self.continuous = false;
        self.preedit.clear();
        self.preedit_started = false;
        self.callback.recording_status(false);
        result
    }

    /// Persist a finalized take as one session + Opus recording (the training
    /// pair's audio + digest) and commit it into the field. A silent take stores
    /// nothing.
    fn finalize_outcome(&mut self, outcome: TakeOutcome) -> Result<(), FfiError> {
        let finalized = match outcome {
            TakeOutcome::Silent => {
                // No usable speech (or no model): clear any live preedit, persist
                // nothing.
                if self.preedit_started {
                    self.callback.cancel_preedit();
                }
                return Ok(());
            }
            TakeOutcome::Speech(finalized) => finalized,
        };
        if let Some(reason) = &finalized.fallback_reason {
            eprintln!("whole-take decode failed at stop; keeping the previewed text: {reason}");
        }
        let segment = segment_from_samples(&finalized.merged_samples);
        let encoded = self.codec.encode(&segment).map_err(|error| FfiError::Io {
            detail: format!("encode audio: {error}"),
        })?;
        let session_id = self
            .dictation
            .storage_mut()
            .create_session(Some(finalized.final_text.as_str()))?;
        let utterance_id = utterance_id_for_session(session_id);
        self.audio_store
            .write_source_audio(&self.user_id, &utterance_id, &encoded)
            .map_err(|error| FfiError::Io {
                detail: format!("write audio: {error}"),
            })?;
        let digest = idiolect_common::digest::audio_sha256_hex(&encoded.payload);
        self.dictation
            .storage_mut()
            .set_audio_digest(&utterance_id, &digest)?;
        // Key the finalize idempotency by the take's unique, persisted session (via its
        // stable utterance id) — never an in-memory counter. A per-process counter resets
        // to 0 on every restart, so the first take after a restart would reuse `...-1` and
        // collide with the persisted idempotency ledger (a conflict that crashes the IME).
        // This mirrors the daemon, which keys this commit by `session_id`.
        let key = format!("ffi-stream-final:{utterance_id}");
        self.dictation
            .commit(session_id, &finalized.final_text, &key)?;
        self.last_commit = Some((session_id, finalized.final_text));
        // Reclaim the oldest captured audio if this take pushed us over the cap, so a
        // phone that never pairs (audio is only dropped after a sync ACK) stays bounded.
        let cap = self.audio_storage_cap_bytes;
        self.dictation.storage_mut().evict_captured_audio_over_cap(
            &self.user_id,
            cap,
            &self.audio_store,
        )?;
        Ok(())
    }
}

/// Build the live timing config from the user's `[vad]` settings. The frame
/// geometry is fixed in the orchestration; only these timings vary.
///
/// `auto_stop_silence_ms` is carried for parity but inert on Android in this
/// increment: it defaults to 0 (disabled), there is no on-device config path to set
/// it, and `push_pcm_frame` does not consult `auto_stop_due()`. The mic is
/// tap-to-toggle; hands-free silence auto-stop (the daemon's `pump_live_stream`
/// behaviour) is a tracked follow-up once a config surface and a pump exist.
fn streaming_config_from(vad: &VadConfig) -> StreamingConfig {
    StreamingConfig {
        min_speech_ms: vad.min_speech_ms,
        pre_roll_ms: vad.pre_roll_ms,
        post_roll_ms: vad.post_roll_ms,
        max_utterance_ms: vad.max_utterance_ms,
        auto_stop_silence_ms: vad.auto_stop_silence_ms,
    }
}

/// Decode threads for on-device whisper. [`WhisperOptions`]'s default of one thread
/// pins inference to a single core, leaving a multi-core phone mostly idle during the
/// compute-bound matmuls that dominate a take's cost. Use the device's available
/// parallelism instead, capped to a sane band so we neither pin to one core nor wildly
/// oversubscribe (extra threads past the physical cores only add fork/join overhead).
fn on_device_decode_threads() -> u32 {
    std::thread::available_parallelism()
        .map(|n| u32::try_from(n.get()).unwrap_or(u32::MAX))
        .unwrap_or(4)
        .clamp(1, 8)
}

/// Wrap raw 16 kHz mono samples as an [`AudioSegment`] for decode/encode.
fn segment_from_samples(samples_f32_mono: &[f32]) -> AudioSegment {
    let duration_ms =
        (samples_f32_mono.len() as u64 * 1_000 / u64::from(STREAM_SAMPLE_RATE_HZ)) as u32;
    AudioSegment {
        sample_rate_hz: STREAM_SAMPLE_RATE_HZ,
        channels: 1,
        duration_ms,
        samples_f32_mono: samples_f32_mono.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        on_device_decode_threads, segment_from_samples, FfiError, HistoryItem,
        STREAM_SAMPLE_RATE_HZ,
    };
    use idiolect_common::ids::ImeSessionId;
    use idiolect_ports::storage::{HistoryEntry, HistoryState};

    #[test]
    fn on_device_decode_threads_uses_available_cores_not_a_single_thread() {
        let threads = on_device_decode_threads();
        assert!(
            (1..=8).contains(&threads),
            "threads must stay in [1, 8]: {threads}"
        );
        // The whole point of this helper is to not pin whisper to one core. On any
        // multi-core host (every CI runner) it must pick more than one decode thread —
        // a regression to the hard-coded `n_threads: 1` would trip this.
        let cores = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1);
        if cores > 1 {
            assert!(
                threads > 1,
                "on a {cores}-core machine whisper must use more than one decode thread, got {threads}",
            );
        }
    }

    #[test]
    fn history_item_maps_state_to_committed_flag() {
        let committed = HistoryEntry {
            id: 1,
            session_id: ImeSessionId::new(),
            text: "hi".to_owned(),
            state: HistoryState::Committed,
            created_at: "now".to_owned(),
        };
        let cancelled = HistoryEntry {
            state: HistoryState::Cancelled,
            ..committed.clone()
        };
        assert!(HistoryItem::from(committed).committed);
        assert!(!HistoryItem::from(cancelled).committed);
    }

    #[test]
    fn storage_error_renders_a_message() {
        let error = FfiError::HistoryEntryNotFound { id: 7 };
        assert!(error.to_string().contains("7"));
    }

    #[test]
    fn segment_from_samples_sets_rate_channels_and_duration() {
        // One second of 16 kHz mono ⇒ 1000 ms, mono, 16 kHz.
        let segment = segment_from_samples(&vec![0.0; STREAM_SAMPLE_RATE_HZ as usize]);
        assert_eq!(segment.sample_rate_hz, STREAM_SAMPLE_RATE_HZ);
        assert_eq!(segment.channels, 1);
        assert_eq!(segment.duration_ms, 1_000);
    }
}
