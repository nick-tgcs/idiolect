# Idiolect

Rust workspace bootstrap and lint baseline for the Idiolect project.

## V1 Verification Gates

All warnings are errors, and any failing command blocks release.

```bash
bash ci/scripts/test-rust.sh
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-integration.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-packaging.sh
```
