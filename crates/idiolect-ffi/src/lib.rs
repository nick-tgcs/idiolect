//! The single UniFFI facade the Android app loads — the in-process replacement
//! for the desktop's Unix-socket daemon.
//!
//! The desktop runs a separate-process daemon and talks to it over a Unix socket
//! with the [`idiolect_ipc`] message vocabulary. On Android there is no daemon:
//! the IME service loads this `.so` and calls the same logic **in process**. So
//! the socket's command/event pairs collapse into:
//!
//! * **Kotlin → Rust**: the methods on [`IdiolectCore`] (`toggle`, `commit`,
//!   `cancel`, `report_correction`, `push_pcm_frame`, history operations).
//! * **Rust → Kotlin**: the [`IdiolectInputMethod`] callback (the analog of the
//!   `RecordingStatus`/`PreeditUpdate`/`InsertText`/`EditHistory` server pushes).
//!
//! The brain itself is reused unchanged: [`IdiolectCore`] drives
//! [`DictationUseCase`] over a real [`SqliteMetadataStore`], exactly as the daemon
//! does. The **streaming decode** (PCM → snippets → transcript) is the shared
//! `idiolect_application::use_cases::streaming::StreamingTake` orchestration that
//! M2 lifted out of the daemon — the daemon already runs on it. M3 wires this
//! facade onto the *same* orchestration: `push_pcm_frame` will feed
//! `StreamingTake::ingest`, an on-device whisper `TakeTranscriber` and a callback
//! `StreamObserver` will drive `fold_snippet`/`finalize`, and the take's session
//! will be created at finalize with the whole-recording text (matching the
//! daemon), not per snippet. That wiring waits for M3's Android audio/ASR/VAD
//! adapters (the orchestration's ports have no Android implementations yet); until
//! then `push_pcm_frame` buffers the capture and [`IdiolectCore::deliver_transcript`]
//! is the test-driven transcript seam.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use idiolect_adapter_sqlite::{SqliteMetadataStore, SqliteStorageError};
use idiolect_application::use_cases::dictation::{DictationUseCase, DictationUseCaseError};
use idiolect_common::config::{PathProvider, RootedPaths};
use idiolect_common::ids::ImeSessionId;
use idiolect_ports::input_method::InputMethodPort;
use idiolect_ports::storage::{HistoryEntry, HistoryState, MetadataStorePort};

uniffi::setup_scaffolding!();

/// Every fallible operation the Android app can trigger maps onto this error so
/// the Kotlin side gets a typed failure rather than a panic across the FFI.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("storage error: {message}")]
    Storage { message: String },
    #[error("no active dictation take")]
    NoActiveTake,
    #[error("history entry {id} not found")]
    HistoryEntryNotFound { id: i64 },
    #[error("io error: {message}")]
    Io { message: String },
}

impl From<SqliteStorageError> for FfiError {
    fn from(error: SqliteStorageError) -> Self {
        Self::Storage {
            message: error.to_string(),
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

/// Mutable state behind the [`IdiolectCore`] mutex.
struct Inner {
    dictation: DictationUseCase<CallbackInput, SqliteMetadataStore>,
    callback: Arc<dyn IdiolectInputMethod>,
    /// Authoritative recording state — the single source of truth (§ daemon).
    recording: bool,
    /// The take currently being dictated, if any.
    active: Option<ActiveTake>,
    /// The most recently committed take, for an in-place [`report_correction`].
    last_commit: Option<(ImeSessionId, String)>,
    /// Monotonic source of per-operation idempotency keys.
    seq: u64,
    /// Raw 16 kHz mono PCM captured for the active take. M3 feeds this through
    /// `StreamingTake::ingest`; until then it proves capture plumbing without decoding.
    pcm: Vec<i16>,
}

/// The in-flight take. The session row is created lazily — only once the take is
/// decoded — because it is seeded with the raw transcript (the raw side of the
/// training pair), which is not known at mic-start. `preedit` is the live text
/// awaiting commit.
struct ActiveTake {
    session_id: Option<ImeSessionId>,
    preedit: Option<String>,
}

/// The in-process core the Android app holds for the life of the IME service.
#[derive(uniffi::Object)]
pub struct IdiolectCore {
    inner: Mutex<Inner>,
}

#[uniffi::export]
impl IdiolectCore {
    /// Open (or create) the on-device store under `data_dir` (the app's private
    /// `filesDir`) and wire the brain to `callback`.
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        callback: Box<dyn IdiolectInputMethod>,
    ) -> Result<Arc<Self>, FfiError> {
        let paths = RootedPaths::new(data_dir);
        std::fs::create_dir_all(paths.data_dir()).map_err(|error| FfiError::Io {
            message: error.to_string(),
        })?;
        let mut store = SqliteMetadataStore::open_path(paths.database_path())?;
        store.migrate()?;
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
                recording: false,
                active: None,
                last_commit: None,
                seq: 0,
                pcm: Vec::new(),
            }),
        }))
    }

    /// One-tap mic key: the core alone decides start-vs-stop from its authoritative
    /// state (the daemon's `ToggleRecording` intent). Each edge pushes
    /// [`IdiolectInputMethod::recording_status`].
    pub fn toggle(&self) -> Result<(), FfiError> {
        let mut inner = self.lock();
        if inner.recording {
            inner.recording = false;
            // M3 consumes the captured take here (drain → `StreamingTake` ingest +
            // finalize → commit); for now the buffer is simply released at stop.
            inner.pcm.clear();
            inner.callback.recording_status(false);
        } else {
            // No session yet: it is created when the take is decoded (see
            // `deliver_transcript`), seeded with the raw transcript.
            inner.active = Some(ActiveTake {
                session_id: None,
                preedit: None,
            });
            inner.recording = true;
            inner.pcm.clear();
            inner.callback.recording_status(true);
        }
        Ok(())
    }

    /// Push one buffer of captured 16 kHz mono PCM (the mic FGS → JNI hot path).
    /// Only valid while a take is recording; a stray frame is rejected rather than
    /// silently buffered.
    pub fn push_pcm_frame(&self, frame: Vec<i16>) -> Result<(), FfiError> {
        let mut inner = self.lock();
        if !inner.recording {
            return Err(FfiError::NoActiveTake);
        }
        inner.pcm.extend_from_slice(&frame);
        Ok(())
    }

    /// Finalise the current take's preedit into the focused field.
    pub fn commit(&self) -> Result<(), FfiError> {
        let mut inner = self.lock();
        let (session_id, text) = {
            let take = inner.active.as_ref().ok_or(FfiError::NoActiveTake)?;
            let session_id = take.session_id.ok_or(FfiError::NoActiveTake)?;
            let text = take.preedit.clone().ok_or(FfiError::NoActiveTake)?;
            (session_id, text)
        };
        let key = inner.next_key("commit");
        inner.dictation.commit(session_id, &text, &key)?;
        inner.last_commit = Some((session_id, text));
        inner.active = None;
        Ok(())
    }

    /// Discard the current take without committing.
    pub fn cancel(&self) -> Result<(), FfiError> {
        let mut inner = self.lock();
        let take = inner.active.take().ok_or(FfiError::NoActiveTake)?;
        match take.session_id {
            // A decoded take has a persisted session to cancel (records a
            // cancelled history row, matching the desktop).
            Some(session_id) => {
                let key = inner.next_key("cancel");
                inner.dictation.cancel(session_id, &key)?;
            }
            // Cancelled before any decode: nothing persisted; just clear the IME.
            None => inner.callback.cancel_preedit(),
        }
        Ok(())
    }

    /// The user fixed the auto-committed text in the field: amend the just-committed
    /// take with the corrected form so a raw→corrected training pair is recorded and
    /// the history projection reflects it. (Streamed-tail merging is M2.)
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
        if let Some(active) = inner.active.as_mut() {
            if active.session_id == Some(entry.session_id) {
                active.preedit = Some(corrected_text);
            }
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
}

// Non-`#[uniffi::export]` seams: callable as ordinary Rust (tests now, M3's
// `StreamingTake`-driven decode later) but not surfaced to Kotlin.
impl IdiolectCore {
    /// Hand a freshly decoded transcript to the active take as its live preedit —
    /// the interim seam M3 replaces with `StreamingTake::fold_snippet`/`finalize`
    /// once an on-device decoder exists. Mirrors the desktop run-loop handing a
    /// transcript to `transcript_ready`.
    pub fn deliver_transcript(&self, text: &str) -> Result<(), FfiError> {
        let mut inner = self.lock();
        let existing = inner
            .active
            .as_ref()
            .ok_or(FfiError::NoActiveTake)?
            .session_id;
        match existing {
            // First decode of the take: create the session row seeded with the raw
            // transcript (commit reads it back as the raw side of the pair) and
            // begin the live preedit.
            None => {
                let session_id = inner.dictation.storage_mut().create_session(Some(text))?;
                inner.dictation.transcript_ready(session_id, text)?;
                let take = inner.active.as_mut().expect("active take present");
                take.session_id = Some(session_id);
                take.preedit = Some(text.to_owned());
            }
            // A later snippet for the same take (M2 streaming) updates in place.
            Some(_) => {
                inner.callback.update_preedit(text.to_owned());
                inner.active.as_mut().expect("active take present").preedit = Some(text.to_owned());
            }
        }
        Ok(())
    }

    /// Number of PCM samples buffered for the active take (M3 drains this).
    #[must_use]
    pub fn buffered_frame_count(&self) -> usize {
        self.lock().pcm.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("idiolect core mutex poisoned")
    }
}

impl Inner {
    /// A fresh idempotency key for a storage operation; monotonic so a retried
    /// commit re-uses the *same* key only when the caller re-uses it (the caller
    /// holds the key for the duration of one logical operation).
    fn next_key(&mut self, op: &str) -> String {
        self.seq += 1;
        format!("ffi-{op}-{}", self.seq)
    }
}

#[cfg(test)]
mod tests {
    use super::{FfiError, HistoryItem};
    use idiolect_common::ids::ImeSessionId;
    use idiolect_ports::storage::{HistoryEntry, HistoryState};

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
}
