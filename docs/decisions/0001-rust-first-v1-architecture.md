# Decision 0001: Rust-First V1 Architecture

Status: Accepted

Idiolect v1 uses Rust for runtime, orchestration, storage, classification, manifest generation, evaluation, promotion, rollback, and required tests. Python may exist only as research reference material under `research/` and is not part of required product operation.

The v1 workspace is split into `idiolect-common`, `idiolect-core`, `idiolect-ports`, `idiolect-application`, adapter crates, `idiolectd`, `idiolect-cli`, `idiolect-test-support`, and `idiolect-integration-tests`.

Rust is pinned to stable `1.96.0`, released 2026-05-28. Rust has no separate LTS channel for this project. Work may proceed in the current `main` checkout because the user explicitly approved that execution mode on 2026-06-04.

Consequences:

- Core crates never expose Fcitx5, whisper-rs, Silero, Opus, rusqlite, Burn, Candle, ONNX Runtime, PyTorch, PEFT, Python, or other backend-specific types.
- Required contract tests are Rust tests.
- The Fcitx5 engine remains a thin C++ shim and communicates through versioned IPC.
- Adapter promotion requires artifact compatibility, metrics, and rollback evidence.
- Non-Rust and third-party backend integrations are adapters only and must communicate through Idiolect-owned interfaces.
