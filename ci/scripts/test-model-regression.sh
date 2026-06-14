#!/usr/bin/env bash
set -euo pipefail

# NOTE: never `--all-features` here. idiolect-trainerctl's only feature is `cuda`
# (-> idiolect-adapter-whisper/cuda, idiolect-trainer-burn/cuda), so `--all-features`
# builds whisper-rs-sys with `-DGGML_CUDA=ON` and fails on the CUDA-less CI runner
# ("Could not find nvcc"). These are CPU regression tests; default features are right.
cargo test -p idiolect-adapter-whisper whisper_transcribes_fixture_audio
cargo test -p idiolect-integration-tests --test real_asr_contracts whisper_transcribes_fixture_audio
cargo test -p idiolect-trainerctl --test manifest_v2 holdout_item_never_appears_in_training_split
cargo test -p idiolect-trainerctl --test evaluation_matrix evaluation_report_contains_master_plan_metrics
