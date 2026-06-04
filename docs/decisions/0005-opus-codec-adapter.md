# Decision 0005: Opus Codec Adapter

Status: Accepted

Idiolect uses `opus = "=0.3.1"` for the real codec adapter. The crate is used with its default feature set.

Native Linux dependency: `libopus-dev` for libopus headers and libraries.

Fixture strategy: sine fixture audio round-trips through encode and decode in Rust integration tests.

Port-isolation reason: Opus stays confined to `idiolect-adapter-opus`; `idiolect-ports` only exposes Idiolect-owned audio DTOs.

Rollback path: if libopus availability or compile stability becomes a blocker, keep the fixture codec adapter as the v1-safe fallback.
