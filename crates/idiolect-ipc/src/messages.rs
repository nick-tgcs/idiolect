use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const FEATURE_PREEDIT: &str = "preedit";
pub const FEATURE_COMMIT: &str = "commit";
/// Opt-in: a client that advertises this feature receives [`IpcMessage::RecordingStatus`]
/// pushes whenever the daemon's authoritative recording state changes (and once
/// right after the handshake). Older clients that do not request it see the exact
/// same byte stream as before.
pub const FEATURE_RECORDING_STATUS: &str = "recording_status";
/// Opt-in: a client that advertises this receives a take-final
/// [`PreeditUpdate`] with `reconcile: true` at the stop of a direct (review-off)
/// streaming take, and is expected to REPLACE the live-typed preview with it
/// (delete what it typed, commit the verified text). Clients that do not
/// advertise it keep the pre-reconcile behaviour — they never get this message,
/// so an older client cannot mistake it for a batch transcript and append it
/// after the preview.
pub const FEATURE_RECONCILE: &str = "reconcile";
/// Opt-in: a client that advertises this receives [`IpcMessage::ActivityStatus`]
/// pushes describing WHICH PHASE of a take the daemon is in — in particular the
/// decode, which happens after the microphone closes but before the transcript
/// exists. [`RecordingStatus`] cannot carry it: its `recording` flag is what the
/// engine's take state machine keys off, and flipping it early would make the
/// engine discard the transcript that follows. This is a strictly additive,
/// presentation-only channel, so a client that does not request it sees exactly
/// the byte stream it always did.
pub const FEATURE_ACTIVITY_STATUS: &str = "activity_status";

const SUPPORTED_FEATURES: [&str; 5] = [
    FEATURE_PREEDIT,
    FEATURE_COMMIT,
    FEATURE_RECORDING_STATUS,
    FEATURE_RECONCILE,
    FEATURE_ACTIVITY_STATUS,
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientHello {
    pub client_name: String,
    pub protocol_version: u16,
    pub features: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerHello {
    pub protocol_version: u16,
    pub accepted_features: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreeditUpdate {
    pub text: String,
    /// When true, the client should let the user review/correct the text in its
    /// own UI before committing (the daemon's "review before insert" mode),
    /// rather than committing immediately. Defaults false for older clients.
    #[serde(default)]
    pub review: bool,
    /// When true, this is a live mid-take snippet (streaming translation): the
    /// client types it into the app and keeps going — no finalize, no review.
    /// The whole take is finalized once, at stop. Defaults false (a take-final
    /// transcript) so daemons/clients that predate streaming interoperate.
    #[serde(default)]
    pub partial: bool,
    /// When true, this take-final transcript should REPLACE the preview the client
    /// already typed live (direct streaming mode): the client deletes its
    /// live-typed run (synthesised backspaces) and commits this verified text
    /// instead, without asking the daemon to commit again (the daemon already owns
    /// the streamed session). Distinguishes a stop-time reconcile from a fresh
    /// batch transcript. Defaults false so older peers interoperate.
    #[serde(default)]
    pub reconcile: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommitPreedit {
    pub text: String,
}

/// An in-place correction of the most recently committed dictation: the user
/// fixed the auto-committed text in the app, and the engine reports the
/// corrected form so the daemon can record a raw→corrected training signal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReportCorrection {
    pub corrected_text: String,
}

/// Server→client request to commit text directly into the focused application
/// at the cursor (used by history re-insert: the daemon asks the active IME
/// front-end to type the stored text where the user is, exactly as a dictation
/// commit would). Unlike [`PreeditUpdate`] this starts no dictation session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InsertText {
    pub text: String,
}

/// Server→client request to open the review/correction dialog seeded with an
/// existing history entry's text, so the user can retroactively fix a take that
/// was committed without live review. Unlike [`InsertText`] it types nothing into
/// the app: the user's edit comes back as [`HistoryEdited`] and only updates the
/// stored record and its raw→corrected training pair.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EditHistory {
    pub id: i64,
    pub text: String,
}

/// Client→server result of an [`EditHistory`] review: the user's corrected text
/// for history entry `id`. The daemon amends the stored entry and rewrites its
/// training pair (raw stays the input, this becomes the target). Sent only when
/// the user confirms; a cancelled dialog sends nothing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoryEdited {
    pub id: i64,
    pub corrected_text: String,
}

/// Server→client push of the daemon's authoritative recording state. The daemon
/// owns the microphone, so this is the single source of truth: adapters mirror it
/// rather than tracking recording state locally. Sent once after the handshake and
/// on every state change, but only to clients that negotiated
/// [`FEATURE_RECORDING_STATUS`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordingStatus {
    pub recording: bool,
}

/// Which phase of a dictation take the daemon is in. Unlike [`RecordingStatus`]
/// this is presentation-only: it tells the front-end what to SHOW the user, and
/// no take state machine keys off it.
///
/// The distinction that matters is `Transcribing`: the microphone has closed but
/// the decode has not finished, which on CPU is seconds of apparent silence. The
/// engine keeps a take "in flight" (`recording = true`) across that window, so
/// without this the user is shown a live-microphone badge while nothing is being
/// recorded, then nothing at all while the machine works.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityPhase {
    /// No take in progress.
    #[default]
    Idle,
    /// The microphone is open and capturing.
    Recording,
    /// The microphone has closed; the captured audio is being decoded.
    Transcribing,
    /// As `Transcribing`, but the speech model still has to be read off disk
    /// first — the cold-start case, which dominates the wait and deserves its own
    /// wording rather than looking like an unusually slow decode.
    LoadingModel,
}

impl ActivityPhase {
    /// Whether a take is in flight in this phase — the value [`RecordingStatus`]
    /// carries. `Transcribing`/`LoadingModel` count: the microphone is shut, but
    /// the take is not finished and its transcript is still to come.
    #[must_use]
    pub fn take_in_flight(self) -> bool {
        !matches!(self, Self::Idle)
    }
}

/// Server→client push of the current [`ActivityPhase`]. Sent only to clients that
/// negotiated [`FEATURE_ACTIVITY_STATUS`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActivityStatus {
    pub phase: ActivityPhase,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorMessage {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoryReinsert {
    pub id: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoryCopy {
    pub id: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoryReinsertResponse {
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoryCopyResponse {
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum IpcMessage {
    ClientHello(ClientHello),
    ServerHello(ServerHello),
    StartRecording,
    StopRecording,
    /// Direction-free "user pressed the toggle key" intent. The daemon alone
    /// decides start-vs-stop from its authoritative state. Preferred over the
    /// explicit `StartRecording`/`StopRecording` pair, which are retained for
    /// older clients.
    ToggleRecording,
    /// Server→client push of the authoritative recording state (see [`RecordingStatus`]).
    RecordingStatus(RecordingStatus),
    /// Server→client push of the take phase for display (see [`ActivityStatus`]).
    ActivityStatus(ActivityStatus),
    PreeditUpdate(PreeditUpdate),
    CommitPreedit(CommitPreedit),
    CancelPreedit,
    ReportCorrection(ReportCorrection),
    /// Client→server: the user's corrected text from an [`EditHistory`] review.
    HistoryEdited(HistoryEdited),
    InsertText(InsertText),
    /// Server→client: open the review dialog over a stored history entry (see [`EditHistory`]).
    EditHistory(EditHistory),
    Error(ErrorMessage),
    HistoryReinsert(HistoryReinsert),
    HistoryCopy(HistoryCopy),
    HistoryReinsertResponse(HistoryReinsertResponse),
    HistoryCopyResponse(HistoryCopyResponse),
}

#[must_use]
pub fn supported_features() -> &'static [&'static str] {
    &SUPPORTED_FEATURES
}

#[cfg(test)]
mod activity_phase_tests {
    use super::{supported_features, ActivityPhase, FEATURE_ACTIVITY_STATUS};

    #[test]
    fn only_idle_means_no_take_is_in_flight() {
        // The decode phases run with the microphone shut but the take unfinished,
        // so they must still count as in-flight: the engine's take state machine
        // is what receives the transcript that follows, and it only accepts one
        // while a take is live.
        assert!(!ActivityPhase::Idle.take_in_flight());
        assert!(ActivityPhase::Recording.take_in_flight());
        assert!(ActivityPhase::Transcribing.take_in_flight());
        assert!(ActivityPhase::LoadingModel.take_in_flight());
    }

    #[test]
    fn activity_status_is_a_negotiable_feature() {
        // Opt-in: without it in the advertised set the daemon must not push
        // ActivityStatus, so an older engine keeps its exact byte stream.
        assert!(supported_features().contains(&FEATURE_ACTIVITY_STATUS));
    }

    #[test]
    fn an_absent_phase_decodes_as_idle() {
        // `phase` defaults so a peer that predates the field is readable.
        assert_eq!(ActivityPhase::default(), ActivityPhase::Idle);
    }
}
