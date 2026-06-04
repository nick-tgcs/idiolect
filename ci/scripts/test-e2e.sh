#!/usr/bin/env bash
set -euo pipefail

cargo test -p idiolect-integration-tests --test dictation_full_stack_fixture --all-features
cargo test -p idiolect-integration-tests --test dictation_full_stack_real_adapters --all-features
cargo test -p idiolect-integration-tests --test learning_pipeline_manifest --all-features
cargo test -p idiolect-integration-tests --test privacy_e2e --all-features
bash ci/scripts/test-fcitx5-integration.sh
