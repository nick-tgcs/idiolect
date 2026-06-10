use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const FEATURE_PREEDIT: &str = "preedit";
pub const FEATURE_COMMIT: &str = "commit";
/// Opt-in: a client that advertises this feature receives [`IpcMessage::RecordingStatus`]
/// pushes whenever the daemon's authoritative recording state changes (and once
/// right after the handshake). Older clients that do not request it see the exact
/// same byte stream as before.
pub const FEATURE_RECORDING_STATUS: &str = "recording_status";

const SUPPORTED_FEATURES: [&str; 3] =
    [FEATURE_PREEDIT, FEATURE_COMMIT, FEATURE_RECORDING_STATUS];

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

/// Server→client push of the daemon's authoritative recording state. The daemon
/// owns the microphone, so this is the single source of truth: adapters mirror it
/// rather than tracking recording state locally. Sent once after the handshake and
/// on every state change, but only to clients that negotiated
/// [`FEATURE_RECORDING_STATUS`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordingStatus {
    pub recording: bool,
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
    PreeditUpdate(PreeditUpdate),
    CommitPreedit(CommitPreedit),
    CancelPreedit,
    ReportCorrection(ReportCorrection),
    InsertText(InsertText),
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
