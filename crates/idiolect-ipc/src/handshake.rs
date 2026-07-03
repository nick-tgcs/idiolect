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

#[cfg(test)]
mod tests {
    use super::{negotiate_protocol, HandshakeError, PROTOCOL_VERSION};
    use crate::messages::{ClientHello, FEATURE_PREEDIT, FEATURE_REPLACE_TAKE};

    fn hello(features: &[&str]) -> ClientHello {
        ClientHello {
            client_name: "test".to_owned(),
            protocol_version: PROTOCOL_VERSION,
            features: features.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    #[test]
    fn replace_take_is_accepted_only_when_the_client_requests_it() {
        // A client that asks for replace_take gets it back.
        let accepted = negotiate_protocol(&hello(&[FEATURE_PREEDIT, FEATURE_REPLACE_TAKE]))
            .expect("handshake")
            .accepted_features;
        assert!(accepted.iter().any(|f| f == FEATURE_REPLACE_TAKE));

        // A client that does not ask for it never receives it — older engines that
        // predate the feature keep the exact prior behaviour.
        let accepted = negotiate_protocol(&hello(&[FEATURE_PREEDIT]))
            .expect("handshake")
            .accepted_features;
        assert!(!accepted.iter().any(|f| f == FEATURE_REPLACE_TAKE));
    }

    #[test]
    fn a_mismatched_protocol_version_is_rejected() {
        let mut hello = hello(&[FEATURE_REPLACE_TAKE]);
        hello.protocol_version = PROTOCOL_VERSION + 1;
        assert_eq!(
            negotiate_protocol(&hello),
            Err(HandshakeError::UnsupportedProtocolVersion(
                PROTOCOL_VERSION + 1
            )),
        );
    }
}
