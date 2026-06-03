# V1 Readiness Amendments

## Required Corrections Before Source Implementation

1. Python is research-only and is excluded from required v1 runtime, tests, training, promotion, rollback, and packaging.
2. Rust contract tests are the required adapter contract mechanism.
3. The authoritative crate topology is `common`, `core`, `ports`, `application`, adapter crates, `idiolectd`, `idiolect-cli`, `test-support`, and `integration-tests`.
4. SQLite storage uses append-only `event_log` plus materialized tables from the first implementation.
5. Adapter promotion requires artifact compatibility metadata and cannot rely on model-quality metrics alone.
6. All dependencies must use pinned versions. Wildcard dependency versions are rejected.
7. Lint warnings are errors. Rust uses `-D warnings`; C++ uses `-Werror`.
8. Work may proceed on `main` for this implementation because the user explicitly approved it on 2026-06-04.
9. Rust is pinned to stable `1.96.0`, released 2026-05-28. Rust has no separate LTS channel for this project.
10. Non-Rust and third-party backends must only appear behind Idiolect-owned interfaces; backend-specific types must not leak into `idiolect-core`, `idiolect-ports`, or `idiolect-application` public APIs.
