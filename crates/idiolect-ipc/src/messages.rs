use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const FEATURE_PREEDIT: &str = "preedit";
pub const FEATURE_COMMIT: &str = "commit";

const SUPPORTED_FEATURES: [&str; 2] = [FEATURE_PREEDIT, FEATURE_COMMIT];

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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommitPreedit {
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorMessage {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum IpcMessage {
    ClientHello(ClientHello),
    ServerHello(ServerHello),
    StartRecording,
    PreeditUpdate(PreeditUpdate),
    CommitPreedit(CommitPreedit),
    CancelPreedit,
    Error(ErrorMessage),
}

#[must_use]
pub fn supported_features() -> &'static [&'static str] {
    &SUPPORTED_FEATURES
}
