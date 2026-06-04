use thiserror::Error;

use crate::messages::{supported_features, ClientHello, ServerHello, PROTOCOL_VERSION};

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum HandshakeError {
    #[error("unsupported protocol version {0}")]
    UnsupportedProtocolVersion(u16),
}

pub fn negotiate_protocol(hello: &ClientHello) -> Result<ServerHello, HandshakeError> {
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(HandshakeError::UnsupportedProtocolVersion(
            hello.protocol_version,
        ));
    }

    let accepted_features = supported_features()
        .iter()
        .filter(|supported| {
            hello
                .features
                .iter()
                .any(|requested| requested.as_str() == **supported)
        })
        .map(|feature| (*feature).to_owned())
        .collect();

    Ok(ServerHello {
        protocol_version: PROTOCOL_VERSION,
        accepted_features,
    })
}
