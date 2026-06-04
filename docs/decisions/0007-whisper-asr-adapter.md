# Decision 0007: Whisper ASR Adapter

Status: Accepted

Idiolect uses `whisper-rs = "=0.16.0"` for the real Whisper ASR adapter with the default feature set.

Native build dependencies: CMake and a C++17 compiler toolchain for the whisper.cpp binding build.

Fixture strategy: repository-managed Whisper model artifact plus deterministic fetch script with pinned URL and SHA-256; integration tests transcribe repository-managed audio fixtures.

Port-isolation reason: whisper-rs stays confined to `idiolect-adapter-whisper`; public APIs only expose Idiolect-owned transcript and capability DTOs.

Rollback path: if the model artifact or build path makes zero-warning gates unstable, keep the fixture ASR adapter as the v1 fallback and defer the real adapter.
