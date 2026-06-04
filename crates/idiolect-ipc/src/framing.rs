use thiserror::Error;

use crate::messages::IpcMessage;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum FramingError {
    #[error("json line is missing newline terminator")]
    MissingTerminator,
    #[error("invalid json line: {0}")]
    InvalidJson(String),
}

pub fn encode_json_line(message: &IpcMessage) -> Result<String, FramingError> {
    let mut line = serde_json::to_string(message)
        .map_err(|error| FramingError::InvalidJson(error.to_string()))?;
    line.push('\n');
    Ok(line)
}

pub fn decode_json_line(line: &str) -> Result<IpcMessage, FramingError> {
    if !line.ends_with('\n') {
        return Err(FramingError::MissingTerminator);
    }

    let without_newline = &line[..line.len() - 1];
    let json = without_newline
        .strip_suffix('\r')
        .unwrap_or(without_newline);

    serde_json::from_str(json).map_err(|error| FramingError::InvalidJson(error.to_string()))
}
