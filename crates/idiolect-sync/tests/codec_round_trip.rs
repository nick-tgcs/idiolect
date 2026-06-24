use std::collections::BTreeMap;

use idiolect_sync::codec::{decode_batch, encode_batch, SyncCodecError};
use idiolect_sync::dto::{SyncBatch, SyncBatchEnvelope, SyncLearning};

fn learning(digest: &str) -> SyncLearning {
    SyncLearning {
        training_candidate_id: 1,
        user_id: "default".to_owned(),
        utterance_id: "utterance:s1".to_owned(),
        text_session_id: "s1".to_owned(),
        audio_object_key: "audio/1970/01/01/default/utterance:s1.ogg".to_owned(),
        audio_digest: digest.to_owned(),
        raw_transcript: "raw".to_owned(),
        corrected_transcript: "corrected".to_owned(),
        source_label: "ime_correction".to_owned(),
        trust_score_bps: 9000,
    }
}

fn envelope() -> SyncBatchEnvelope {
    let mut audio = BTreeMap::new();
    audio.insert("aa".repeat(32), vec![0u8, 1, 2, 3, 0xff]);
    audio.insert("bb".repeat(32), vec![9u8; 100]);
    let batch = SyncBatch {
        device_id: "pixel-graphene".to_owned(),
        batch_id: "batch-0001".to_owned(),
        // Three learnings, two distinct digests: the repeated one must not
        // duplicate the audio bytes.
        learnings: vec![
            learning(&"aa".repeat(32)),
            learning(&"bb".repeat(32)),
            learning(&"aa".repeat(32)),
        ],
    };
    SyncBatchEnvelope::new(batch, audio)
}

#[test]
fn envelope_round_trips_through_the_codec() {
    let env = envelope();
    let bytes = encode_batch(&env).expect("encode");
    let decoded = decode_batch(&bytes).expect("decode");
    assert_eq!(decoded, env);
    // Content-addressed: the digest shared by two learnings stored its bytes once.
    assert_eq!(decoded.audio.len(), 2);
    assert!(decoded.missing_audio_digests().is_empty());
}

#[test]
fn empty_audio_envelope_round_trips() {
    let env = SyncBatchEnvelope::new(
        SyncBatch {
            device_id: "d".to_owned(),
            batch_id: "b".to_owned(),
            learnings: vec![],
        },
        BTreeMap::new(),
    );
    let bytes = encode_batch(&env).expect("encode");
    assert_eq!(decode_batch(&bytes).expect("decode"), env);
}

#[test]
fn wrong_magic_is_rejected() {
    let err = decode_batch(b"NOPEEEE and some more bytes here").expect_err("bad magic");
    assert!(matches!(err, SyncCodecError::BadMagic), "got {err:?}");
}

#[test]
fn truncated_payload_is_rejected() {
    let bytes = encode_batch(&envelope()).expect("encode");
    let err = decode_batch(&bytes[..bytes.len() - 5]).expect_err("truncated");
    assert!(matches!(err, SyncCodecError::UnexpectedEof), "got {err:?}");
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut bytes = encode_batch(&envelope()).expect("encode");
    bytes.push(0);
    let err = decode_batch(&bytes).expect_err("trailing");
    assert!(matches!(err, SyncCodecError::TrailingBytes), "got {err:?}");
}
