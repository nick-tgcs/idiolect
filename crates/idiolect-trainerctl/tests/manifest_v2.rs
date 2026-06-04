use idiolect_trainerctl::{
    CandidateLabel, LearningManifestBuilder, ManifestSplit, ManifestV2BuildInput,
    ManifestV2CandidateInput,
};

#[test]
fn manifest_v2_contains_train_validation_and_holdout_splits() {
    let manifest = LearningManifestBuilder::build_v2(ManifestV2BuildInput::new(
        "default",
        "whisper-medium-en",
        (0..10)
            .map(|index| approved_candidate(&format!("candidate-{index:02}"), "audio-digest"))
            .collect(),
    ))
    .expect("manifest should build");

    assert!(manifest
        .items()
        .iter()
        .any(|item| item.split() == ManifestSplit::Train));
    assert!(manifest
        .items()
        .iter()
        .any(|item| item.split() == ManifestSplit::Validation));
    assert!(manifest
        .items()
        .iter()
        .any(|item| item.split() == ManifestSplit::Holdout));
}

#[test]
fn holdout_item_never_appears_in_training_split() {
    let manifest = LearningManifestBuilder::build_v2(ManifestV2BuildInput::new(
        "default",
        "whisper-medium-en",
        (0..10)
            .map(|index| approved_candidate(&format!("candidate-{index:02}"), "audio-digest"))
            .collect(),
    ))
    .expect("manifest should build");

    let holdout_ids = manifest
        .items_for_split(ManifestSplit::Holdout)
        .iter()
        .map(|item| item.training_candidate_id())
        .collect::<Vec<_>>();
    let training_ids = manifest
        .items_for_split(ManifestSplit::Train)
        .iter()
        .map(|item| item.training_candidate_id())
        .collect::<Vec<_>>();

    assert!(!holdout_ids.is_empty());
    for holdout_id in holdout_ids {
        assert!(!training_ids.contains(&holdout_id));
    }
}

#[test]
fn manifest_item_links_audio_and_text_session() {
    let manifest = LearningManifestBuilder::build_v2(ManifestV2BuildInput::new(
        "default",
        "whisper-medium-en",
        vec![approved_candidate("candidate-01", "audio-digest-01")],
    ))
    .expect("manifest should build");

    let item = manifest.items().first().expect("manifest item");
    assert_eq!(item.user_id(), "default");
    assert_eq!(item.utterance_id(), "utterance-candidate-01");
    assert_eq!(item.text_session_id(), "session-candidate-01");
    assert_eq!(
        item.audio_object_key(),
        "audio/1970/01/01/default/utterance-candidate-01.ogg"
    );
    assert_eq!(item.audio_digest(), "audio-digest-01");
    assert_eq!(item.raw_transcript(), "candidate-01 raw");
    assert_eq!(item.corrected_transcript(), "candidate-01 corrected");
    assert_eq!(item.source_label(), "ime_preedit_correction");
    assert_eq!(item.trust_score_bps(), 10_000);
    assert_eq!(item.base_model_id(), "whisper-medium-en");
}

#[test]
fn manifest_digest_changes_when_audio_digest_changes() {
    let first = LearningManifestBuilder::build_v2(ManifestV2BuildInput::new(
        "default",
        "whisper-medium-en",
        vec![approved_candidate("candidate-01", "audio-digest-01")],
    ))
    .expect("first manifest should build");
    let second = LearningManifestBuilder::build_v2(ManifestV2BuildInput::new(
        "default",
        "whisper-medium-en",
        vec![approved_candidate("candidate-01", "audio-digest-02")],
    ))
    .expect("second manifest should build");

    assert_ne!(first.digest(), second.digest());
}

fn approved_candidate(id: &str, audio_digest: &str) -> ManifestV2CandidateInput {
    ManifestV2CandidateInput {
        training_candidate_id: id.to_owned(),
        user_id: "default".to_owned(),
        utterance_id: format!("utterance-{id}"),
        text_session_id: format!("session-{id}"),
        audio_object_key: format!("audio/1970/01/01/default/utterance-{id}.ogg"),
        audio_digest: audio_digest.to_owned(),
        raw_transcript: format!("{id} raw"),
        corrected_transcript: format!("{id} corrected"),
        source_label: "ime_preedit_correction".to_owned(),
        label: CandidateLabel::Approved {
            trust_score_bps: 10_000,
        },
    }
}
