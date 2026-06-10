#!/usr/bin/env bash
set -euo pipefail

cargo test -p idiolect-integration-tests --all-features --test daemon_run_lifecycle daemon_run_cancel_does_not_commit_text
cargo test -p idiolect-integration-tests --all-features --test daemon_run_lifecycle daemon_run_retry_does_not_duplicate_committed_session

cargo test -p idiolect-integration-tests --all-features --test daemon_run_lifecycle daemon_disconnect_marks_session_abandoned
cargo test -p idiolect-integration-tests --all-features --test daemon_run_lifecycle daemon_run_unsupported_asr_engine_returns_safe_error
cargo test -p idiolect-integration-tests --all-features --test storage_lifecycle lifecycle_commit_is_replay_consistent_after_restart
cargo test -p idiolect-application --all-features retry_after_input_commit_failure_replays_commit
cargo test -p idiolect-adapter-whisper whisper_reports_typed_error_for_invalid_fixture_model
cargo test -p idiolectd --test config_runtime idiolectd_run_rejects_missing_model_path
