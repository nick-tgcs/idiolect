# Decision 0006: Fast VAD Adapter

Status: Accepted

Idiolect uses `fast-vad = "=0.2.1"` for the real VAD adapter. The crate is used with its default feature set. `fast-vad` supports 8 kHz and 16 kHz mono audio and fixed 32 ms frames; v1 uses 16 kHz mono.

Native build/runtime dependencies: Python 3.12 headers, `numpy 1.26.4`, `pyo3-build-config`, and the standard Rust toolchain already present on the machine. The repository keeps the Python runtime out of the product API; it is a build dependency for the adapter crate only.

Fixture strategy: pure Rust test-support audio fixtures, including speech-and-silence samples at 16 kHz, with adapter tests asserting one deterministic speech region on the fixture.

Port-isolation reason: the VAD backend stays confined to `idiolect-adapter-vad`; `idiolect-core`, `idiolect-ports`, and `idiolect-application` expose only Idiolect-owned DTOs and traits.

Rollback path: if `fast-vad` breaks zero-warning build/test gates or the Python build dependency becomes unstable, replace it with a non-Python VAD crate such as `webrtc-vad`.
