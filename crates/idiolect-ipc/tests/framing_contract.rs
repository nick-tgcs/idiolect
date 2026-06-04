use idiolect_ipc::framing::{decode_json_line, encode_json_line, FramingError};
use idiolect_ipc::messages::{ClientHello, IpcMessage};

#[test]
fn json_lines_round_trip_message_category() {
    let message = IpcMessage::ClientHello(ClientHello {
        client_name: "idiolect-fcitx5".to_owned(),
        protocol_version: 1,
        features: vec!["preedit".to_owned(), "commit".to_owned()],
    });

    let line = encode_json_line(&message).expect("message should encode");

    assert!(line.ends_with('\n'));
    let decoded = decode_json_line(&line).expect("message should decode");
    assert_eq!(decoded, message);
}

#[test]
fn decoding_without_json_line_terminator_is_rejected() {
    let error = decode_json_line(r#"{"type":"CancelPreedit"}"#)
        .expect_err("missing newline should be rejected");

    assert_eq!(error, FramingError::MissingTerminator);
}
