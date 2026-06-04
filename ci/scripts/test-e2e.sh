#!/usr/bin/env bash
set -euo pipefail

cargo test -p idiolect-integration-tests --test dictation_full_stack_fixture --all-features
cargo test -p idiolect-integration-tests --test dictation_full_stack_real_adapters --all-features
