#!/usr/bin/env bash
set -euo pipefail

bash ci/scripts/test-rust.sh
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-e2e.sh
cargo test -p idiolect-integration-tests --all-targets --all-features
cargo test -p idiolect-cli --tests
