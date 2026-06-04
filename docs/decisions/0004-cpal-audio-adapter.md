# Decision 0004: CPAL Audio Adapter

Status: Accepted

Idiolect uses `cpal = "=0.17.3"` for the real Linux audio input adapter. The crate is used with its default feature set.

Native Linux dependency: `libasound2-dev` for ALSA headers and libraries. The build target for v1 is Linux only.

Fixture strategy: deterministic unit and integration tests use named-device negative-path coverage and fixture-backed audio data, not hardware-only validation.

Port-isolation reason: CPAL stays confined to `idiolect-adapter-cpal`; `idiolect-core`, `idiolect-ports`, and `idiolect-application` never expose device or stream types.

Rollback path: if CPAL causes zero-warning build or test instability, retain the fixture audio adapter and defer the real adapter.
