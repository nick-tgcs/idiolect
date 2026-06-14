use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_ports::storage::MetadataStorePort;
use idiolect_trainerctl::{
    CandidateLabel, LearningManifestBuilder, ManifestSplit, ManifestV2BuildInput,
    ManifestV2CandidateInput,
};

#[test]
fn manifest_builder_exports_valid_v2_splits_from_sqlite_candidates() {
    let mut store = SqliteMetadataStore::open_in_memory().expect("store should open");
    store.migrate().expect("migrations should run");

    for index in 0..10 {
        let session_id = store
            .create_session(Some(&format!("raw transcript {index}")))
            .expect("session should create");
        store
            .record_preedit_change(
                session_id,
                &format!("raw transcript {index}"),
                &format!("corrected transcript {index}"),
                0,
            )
            .expect("preedit change should record");
        store
            .commit_session(
                session_id,
                &format!("corrected transcript {index}"),
                &format!("manifest-builder-commit-{index}"),
            )
            .expect("session should commit");

        let link = store
            .session_utterance_link_for_test(session_id)
            .expect("link should query")
            .expect("link should exist");
        // Use the production digest path with a real content hash, so this proves
        // a genuinely-computed digest (not a fabricated string) flows through
        // build_v2 — the same code capture now runs.
        let digest =
            idiolect_common::digest::audio_sha256_hex(format!("opus-payload-{index}").as_bytes());
        store
            .set_audio_digest(&link.utterance_id, &digest)
            .expect("audio digest should persist");
    }

    let inputs = store
        .training_candidates_for_manifest_v2("default")
        .expect("manifest candidates should query")
        .into_iter()
        .map(|candidate| ManifestV2CandidateInput {
            training_candidate_id: candidate.training_candidate_id.to_string(),
            user_id: candidate.user_id,
            utterance_id: candidate.utterance_id,
            text_session_id: candidate.text_session_id,
            audio_object_key: candidate.audio_object_key,
            audio_digest: candidate.audio_digest,
            raw_transcript: candidate.raw_transcript,
            corrected_transcript: candidate.corrected_transcript,
            source_label: candidate.source_label,
            label: CandidateLabel::Approved {
                trust_score_bps: candidate.trust_score_bps,
            },
        })
        .collect();

    let manifest = LearningManifestBuilder::build_v2(ManifestV2BuildInput::new(
        "default",
        "whisper-medium-en",
        inputs,
    ))
    .expect("manifest should build");

    assert_eq!(manifest.items().len(), 10);
    assert_eq!(manifest.items_for_split(ManifestSplit::Holdout).len(), 1);
    assert_eq!(manifest.items_for_split(ManifestSplit::Validation).len(), 1);
    assert_eq!(manifest.items_for_split(ManifestSplit::Train).len(), 8);
    assert!(manifest
        .items()
        .iter()
        .all(|item| !item.audio_digest().is_empty()));
    assert!(manifest
        .items()
        .iter()
        .all(|item| item.base_model_id() == "whisper-medium-en"));
}
