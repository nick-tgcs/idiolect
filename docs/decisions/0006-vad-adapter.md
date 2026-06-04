# Decision 0006: WebRTC VAD Adapter

Status: Accepted

Idiolect uses `webrtc-vad = "=0.4.0"` for the real VAD adapter. The adapter runs at 16 kHz mono, processes fixed 30 ms frames, and merges short internal gaps so one spoken phrase remains one deterministic segment for fixtures.

Native build/runtime dependencies: a C compiler through the crate's `cc` build dependency. The crate packages the libfvad C sources in the published crate; normal Cargo builds do not fetch git submodules or require Python.

Fixture strategy: pure Rust test-support audio fixtures, including speech-and-silence samples at 16 kHz, with adapter tests asserting one deterministic speech region on the fixture.

Port-isolation reason: the VAD backend stays confined to `idiolect-adapter-vad`; `idiolect-core`, `idiolect-ports`, and `idiolect-application` expose only Idiolect-owned DTOs and traits.

Rollback path: if `webrtc-vad` breaks zero-warning build/test gates or C compilation becomes unsuitable for packaging, replace it with a pure-Rust VAD implementation behind the same `VadPort` boundary.
