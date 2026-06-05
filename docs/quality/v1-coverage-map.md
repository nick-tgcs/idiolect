# Prototype Baseline And Acceptance Gate Evidence Map

This map records prototype baseline evidence and local acceptance gate coverage. It is not master-plan v1 completion evidence, and it does not claim child plans 00-07 satisfy the full master plan.

| process | automated_test |
| --- | --- |
| audio.capture | real_media_full_stack_transcribes_fixture_and_commits_candidate |
| audio.fixture | fixture_pipeline_contract_is_deterministic_end_to_end |
| codec.opus | opus_codec_round_trips_fixture_metadata |
| vad.segment | vad_contract_returns_one_segment_for_fixture |
| asr.whisper | whisper_transcribes_fixture_audio |
| daemon.startup | idiolectd_version_reports_json |
| ipc.handshake | fcitx5_client_protocol_version_is_accepted |
| ipc.lifecycle | fixture_full_stack_commit_records_preedit_commit_storage_and_candidate |
| fcitx5.preedit | e2e_ipc_bridge_test |
| fcitx5.commit | e2e_ipc_bridge_test |
| fcitx5.cancel | e2e_ipc_bridge_test |
| storage.event_log | migration_01_creates_event_log |
| storage.materialized_tables | migration_01_creates_materialized_tables |
| candidate.capture | fake_dictation_loop_corrects_and_commits_one_session |
| learning.classifier | candidate_capture_classifier_manifest_promotion_and_rollback_are_connected |
| learning.manifest | candidate_capture_classifier_manifest_promotion_and_rollback_are_connected |
| learning.promotion | candidate_capture_classifier_manifest_promotion_and_rollback_are_connected |
| learning.rollback | candidate_capture_classifier_manifest_promotion_and_rollback_are_connected |
| privacy.export | privacy_export_reports_json_user |
| privacy.delete | privacy_delete_removes_audio_text_events_candidates_cache_and_manifest_refs |
| privacy.deleted_data_excluded | strict_privacy_excludes_deleted_sample_from_future_adapter |
| package.payload | ci/scripts/test-packaging.sh |
| package.smoke | ci/scripts/test-package-smoke.sh |
| package.lifecycle | ci/scripts/test-package-lifecycle.sh |
| e2e.failure_recovery | ci/scripts/test-e2e-failure-recovery.sh |
| model.regression | ci/scripts/test-model-regression.sh |
| performance.smoke | ci/scripts/test-performance.sh |
| acceptance.evidence | ci/scripts/test-coverage-map.sh |
