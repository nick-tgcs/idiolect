# Prototype Baseline Evidence Map

This map records prototype baseline evidence only. It is not master-plan v1 completion evidence, and it does not claim child plans 00-07 satisfy the full master plan.
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
| privacy.delete | privacy_delete_removes_user_materialized_data_and_appends_event |
| privacy.deleted_data_excluded | privacy_delete_removes_training_data_and_future_manifest_excludes_user |
| package.payload | ci/scripts/test-packaging.sh |
| package.smoke | ci/scripts/test-package-smoke.sh |
