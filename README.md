# Idiolect

Rust workspace bootstrap and lint baseline for the Idiolect project.

## Status

This repository is currently a prototype baseline and not yet Idiolect v1 complete.

## Baseline Verification Gates

All warnings are errors, and any failing command blocks the current baseline. These checks do not prove v1 completion. Run the full baseline gate:

```bash
bash ci/scripts/test-all.sh
```

Direct gates run by `test-all.sh`:

```bash
bash ci/scripts/test-rust.sh
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-integration.sh
bash ci/scripts/test-e2e.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-packaging.sh
bash ci/scripts/test-package-smoke.sh
bash ci/scripts/test-coverage-map.sh
bash ci/scripts/test-coverage.sh
```
