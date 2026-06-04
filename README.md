# Idiolect

Rust workspace bootstrap and lint baseline for the Idiolect project.

## V1 Verification Gates

All warnings are errors, and any failing command blocks release. Run the full release gate:

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
