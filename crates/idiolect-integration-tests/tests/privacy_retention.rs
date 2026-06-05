use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::repository::{
    AdapterRegistration, AdapterRegistrationInput, PrivacyRetentionMode,
};
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioRetentionMode, AudioStorePort, MetadataStorePort};
use idiolect_test_support::fixtures::restart_traffic_fixture_16khz_mono;

#[test]
fn privacy_delete_removes_audio_text_events_candidates_cache_and_manifest_refs() {
    let fixture = PrivacyFixture::new("privacy-delete-all");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();
    let (_utterance_id, audio_ref, cache_ref, candidate_id) =
        populate_private_session(&mut store, &audio_store);
    store
        .insert_manifest_item_for_test("default", "manifest-private", candidate_id)
        .expect("manifest item should be inserted");

    store
        .delete_user_data_with_retention_for_test(
            "default",
            &audio_store,
            PrivacyRetentionMode::Minimal,
        )
        .expect("privacy delete should remove db rows and files");

    let counts = store
        .private_row_counts_for_test("default")
        .expect("private row counts should query");
    assert_eq!(counts.utterances, 0);
    assert_eq!(counts.text_sessions, 0);
    assert_eq!(counts.edit_events, 0);
    assert_eq!(counts.training_candidates, 0);
    assert_eq!(counts.manifest_items, 0);
    assert!(!audio_store
        .source_audio_exists_for_test(&audio_ref)
        .expect("source audio existence should query"));
    assert!(!audio_store
        .decoded_cache_exists_for_test(&cache_ref)
        .expect("decoded cache existence should query"));
}

#[test]
fn strict_privacy_excludes_deleted_sample_from_future_adapter() {
    let fixture = PrivacyFixture::new("strict-deleted-sample");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();
    let (_utterance_id, _audio_ref, _cache_ref, candidate_id) =
        populate_private_session(&mut store, &audio_store);
    store
        .register_adapter_candidate(
            adapter_registration("strict-derived-adapter", candidate_id)
                .with_training_candidate_id(candidate_id),
        )
        .expect("adapter should register");

    store
        .delete_training_candidate_for_privacy_for_test(
            "default",
            candidate_id,
            PrivacyRetentionMode::StrictPrivate,
        )
        .expect("strict delete should remove sample and mark adapters");

    assert!(store
        .training_candidates_for_manifest_v2("default")
        .expect("future manifest candidates should query")
        .is_empty());
    let snapshot = store
        .adapter_registry_snapshot("default")
        .expect("adapter snapshot should query");
    assert!(snapshot
        .entry("strict-derived-adapter")
        .expect("adapter should remain registered")
        .derived_from_deleted_sample());
}

#[test]
fn retention_modes_delete_decoded_cache_immediately_and_apply_source_audio_policy() {
    let minimal = RetentionCase::new("minimal", AudioRetentionMode::Minimal, false);
    minimal.apply_and_assert();

    let balanced = RetentionCase::new("balanced", AudioRetentionMode::Balanced, true);
    balanced.apply_and_assert();

    let research = RetentionCase::new("research", AudioRetentionMode::Research, true);
    research.apply_and_assert();
}

#[test]
fn normal_typing_outside_idiolect_session_is_not_stored() {
    let fixture = PrivacyFixture::new("normal-typing");
    let store = fixture.open_store();
    let counts = store
        .private_row_counts_for_test("default")
        .expect("private row counts should query");

    assert_eq!(counts.utterances, 0);
    assert_eq!(counts.text_sessions, 0);
    assert_eq!(counts.edit_events, 0);
    assert_eq!(counts.training_candidates, 0);
}

#[test]
fn logs_do_not_include_private_text_by_default() {
    let private = "private corrected transcript";
    let redacted = idiolectd::runtime::redact_observability_line_for_test(
        &format!("transcript={private}"),
        false,
    );
    assert!(!redacted.contains(private));
    assert!(redacted.contains("[redacted]"));

    let included = idiolectd::runtime::redact_observability_line_for_test(
        &format!("transcript={private}"),
        true,
    );
    assert!(included.contains(private));
}

struct RetentionCase {
    mode_name: &'static str,
    mode: AudioRetentionMode,
    keeps_source_audio: bool,
}

impl RetentionCase {
    fn new(mode_name: &'static str, mode: AudioRetentionMode, keeps_source_audio: bool) -> Self {
        Self {
            mode_name,
            mode,
            keeps_source_audio,
        }
    }

    fn apply_and_assert(&self) {
        let fixture = PrivacyFixture::new(self.mode_name);
        let mut store = fixture.open_store();
        let audio_store = fixture.audio_store();
        let (_utterance_id, audio_ref, cache_ref, _candidate_id) =
            populate_private_session(&mut store, &audio_store);

        audio_store
            .apply_retention(&audio_ref, &cache_ref, self.mode)
            .expect("retention policy should apply");

        assert_eq!(
            audio_store
                .source_audio_exists_for_test(&audio_ref)
                .expect("source audio existence should query"),
            self.keeps_source_audio
        );
        assert!(!audio_store
            .decoded_cache_exists_for_test(&cache_ref)
            .expect("decoded cache existence should query"));
    }
}

fn populate_private_session(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
) -> (
    String,
    idiolect_ports::storage::AudioObjectRef,
    idiolect_ports::storage::DecodedAudioCacheRef,
    i64,
) {
    let raw_text = "restart traffic";
    let corrected_text = "restart Traefik";
    let session_id = store
        .create_session(Some(raw_text))
        .expect("session should be created");
    store
        .record_preedit_change(session_id, raw_text, corrected_text, 0)
        .expect("correction should be recorded");
    store
        .commit_session(session_id, corrected_text, "privacy-retention-commit")
        .expect("session should commit");
    let utterance_id = store
        .session_utterance_link_for_test(session_id)
        .expect("session link should query")
        .expect("session should have utterance")
        .utterance_id;

    let segment = restart_traffic_fixture_16khz_mono();
    let codec = OpusCodec::new();
    let encoded = codec.encode(&segment).expect("fixture should encode");
    let audio_ref = audio_store
        .write_source_audio("default", &utterance_id, &encoded)
        .expect("source audio should write");
    let cache_ref = audio_store
        .write_decoded_cache("default", &utterance_id, &segment)
        .expect("decoded cache should write");
    let candidate_id = store
        .training_candidates_for_manifest("default")
        .expect("training candidates should query")
        .first()
        .expect("candidate should exist")
        .id;

    (utterance_id, audio_ref, cache_ref, candidate_id)
}

fn adapter_registration(adapter_id: &str, training_candidate_id: i64) -> AdapterRegistration {
    AdapterRegistration::new(AdapterRegistrationInput {
        user_id: "default".to_owned(),
        adapter_id: adapter_id.to_owned(),
        artifact_digest: format!("artifact-{training_candidate_id}"),
        manifest_digest: format!("manifest-{training_candidate_id}"),
        metric_report_digest: format!("metrics-{training_candidate_id}"),
        base_model: "whisper-medium-en".to_owned(),
        adapter_path: format!("adapters/{adapter_id}"),
        metrics: "{\"wer_personal_delta\":-0.08}".to_owned(),
    })
}

struct PrivacyFixture {
    root: PathBuf,
}

impl PrivacyFixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = env::temp_dir().join(format!(
            "idiolect-privacy-retention-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root should be created");
        Self { root }
    }

    fn open_store(&self) -> SqliteMetadataStore {
        let mut store = SqliteMetadataStore::open_path(self.root.join("idiolect.sqlite"))
            .expect("store should open");
        store.migrate().expect("store should migrate");
        store
    }

    fn audio_store(&self) -> FileAudioStore {
        FileAudioStore::new(self.root.join("audio"), self.root.join("decoded"))
    }
}

impl Drop for PrivacyFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
