use idiolect_ipc::handshake::{negotiate_protocol, HandshakeError};
use idiolect_ipc::messages::ClientHello;

#[test]
fn fcitx5_client_protocol_version_is_accepted() {
    let hello = ClientHello {
        client_name: "idiolect-fcitx5".to_owned(),
        protocol_version: 1,
        features: vec!["preedit".to_owned(), "commit".to_owned()],
    };

    let response = negotiate_protocol(&hello).expect("protocol version 1 should be accepted");

    assert_eq!(response.protocol_version, 1);
    assert_eq!(
        response.accepted_features,
        vec!["preedit".to_owned(), "commit".to_owned()]
    );
}

#[test]
fn recording_status_feature_is_accepted_when_requested() {
    let hello = ClientHello {
        client_name: "idiolect-ibus".to_owned(),
        protocol_version: 1,
        features: vec![
            "preedit".to_owned(),
            "commit".to_owned(),
            "recording_status".to_owned(),
        ],
    };

    let response = negotiate_protocol(&hello).expect("protocol version 1 should be accepted");

    assert!(
        response
            .accepted_features
            .contains(&"recording_status".to_owned()),
        "expected recording_status to be negotiated, got {:?}",
        response.accepted_features
    );
}

#[test]
fn recording_status_feature_absent_when_not_requested() {
    let hello = ClientHello {
        client_name: "idiolect-fcitx5".to_owned(),
        protocol_version: 1,
        features: vec!["preedit".to_owned(), "commit".to_owned()],
    };

    let response = negotiate_protocol(&hello).expect("protocol version 1 should be accepted");

    assert!(
        !response
            .accepted_features
            .contains(&"recording_status".to_owned()),
        "recording_status must not be granted unless requested"
    );
}

#[test]
fn unknown_protocol_version_is_rejected() {
    let hello = ClientHello {
        client_name: "idiolect-fcitx5".to_owned(),
        protocol_version: 99,
        features: vec!["preedit".to_owned(), "commit".to_owned()],
    };

    let error = negotiate_protocol(&hello).expect_err("protocol version 99 should be rejected");

    assert_eq!(error, HandshakeError::UnsupportedProtocolVersion(99));
}
