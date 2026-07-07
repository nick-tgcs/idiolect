#!/usr/bin/env bash
set -euo pipefail

bash ci/scripts/test-rust.sh
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-integration.sh
bash ci/scripts/test-e2e.sh
bash ci/scripts/test-e2e-failure-recovery.sh
bash ci/scripts/test-model-regression.sh
bash ci/scripts/test-performance.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-gui-crates-build-standalone.sh
bash ci/scripts/test-packaging.sh
bash ci/scripts/test-package-smoke.sh
bash ci/scripts/test-package-lifecycle.sh
bash ci/scripts/test-coverage-map.sh
bash ci/scripts/test-coverage.sh
