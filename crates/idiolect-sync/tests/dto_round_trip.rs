use idiolect_sync::dto::{SyncBatch, SyncLearning};

fn sample_learning(n: u8) -> SyncLearning {
    SyncLearning {
        training_candidate_id: i64::from(n),
        user_id: "default".to_owned(),
        utterance_id: format!("utterance:s{n}"),
        text_session_id: format!("s{n}"),
        audio_object_key: format!("audio/1970/01/01/default/utterance:s{n}.ogg"),
        audio_digest: format!("{n:064x}"),
        raw_transcript: format!("raw {n}"),
        corrected_transcript: format!("corrected {n}"),
        source_label: "ime_correction".to_owned(),
        trust_score_bps: 9000,
    }
}

#[test]
fn sync_batch_round_trips_through_json() {
    let batch = SyncBatch {
        device_id: "pixel-graphene".to_owned(),
        batch_id: "batch-0001".to_owned(),
        learnings: vec![sample_learning(1), sample_learning(2)],
    };
    let json = serde_json::to_string(&batch).expect("serialize");
    let decoded: SyncBatch = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, batch);
}

#[test]
fn sync_learning_has_no_split_field() {
    // The PC owns train/val/holdout; the phone must never ship a split decision.
    let json = serde_json::to_string(&sample_learning(1)).expect("serialize");
    assert!(
        !json.contains("split"),
        "wire learning must omit split: {json}"
    );
}
