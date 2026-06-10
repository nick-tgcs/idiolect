#!/usr/bin/env bash
set -euo pipefail

cargo test -p idiolect-adapter-whisper whisper_transcribes_fixture_audio
cargo test -p idiolect-integration-tests --all-features --test real_asr_contracts whisper_transcribes_fixture_audio
cargo test -p idiolect-trainerctl --all-features --test manifest_v2 holdout_item_never_appears_in_training_split
cargo test -p idiolect-trainerctl --all-features --test evaluation_matrix evaluation_report_contains_master_plan_metrics
