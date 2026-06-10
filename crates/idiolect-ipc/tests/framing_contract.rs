use idiolect_ipc::framing::{decode_json_line, encode_json_line, FramingError};
use idiolect_ipc::messages::{ClientHello, InsertText, IpcMessage, RecordingStatus};

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
fn insert_text_round_trips_over_the_wire() {
    // The daemon asks the active IME front-end to type a stored history entry at
    // the cursor; the message must survive the newline-JSON framing intact.
    let message = IpcMessage::InsertText(InsertText {
        text: "Deploy traefik and nginx".to_owned(),
    });

    let line = encode_json_line(&message).expect("message should encode");
    assert!(line.ends_with('\n'));
    assert_eq!(decode_json_line(&line).expect("decode"), message);
}

#[test]
fn toggle_recording_round_trips_over_the_wire() {
    // The direction-free "user pressed the toggle key" intent: the adapter sends
    // it and the daemon alone decides start-vs-stop.
    let message = IpcMessage::ToggleRecording;

    let line = encode_json_line(&message).expect("message should encode");
    assert!(line.ends_with('\n'));
    // The C++ fcitx5 client string-matches on this exact tag.
    assert_eq!(line.trim_end(), r#"{"type":"ToggleRecording"}"#);
    assert_eq!(decode_json_line(&line).expect("decode"), message);
}

#[test]
fn recording_status_round_trips_over_the_wire() {
    // Daemon→client push of the authoritative recording state.
    for recording in [true, false] {
        let message = IpcMessage::RecordingStatus(RecordingStatus { recording });

        let line = encode_json_line(&message).expect("message should encode");
        assert!(line.ends_with('\n'));
        assert_eq!(
            line.trim_end(),
            format!(r#"{{"type":"RecordingStatus","payload":{{"recording":{recording}}}}}"#)
        );
        assert_eq!(decode_json_line(&line).expect("decode"), message);
    }
}

#[test]
fn decoding_without_json_line_terminator_is_rejected() {
    let error = decode_json_line(r#"{"type":"CancelPreedit"}"#)
        .expect_err("missing newline should be rejected");

    assert_eq!(error, FramingError::MissingTerminator);
}
