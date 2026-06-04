# Idiolect 06 Fcitx5 CLI Packaging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the Linux user-facing surfaces: Fcitx5 thin shim, CLI privacy and diagnostic commands, integration gates, and installable packaging artifacts.

**Architecture:** Fcitx5 remains a thin C++ client that speaks versioned IPC to `idiolectd`; product behavior stays in Rust. The CLI operates through Rust application/storage APIs and packaging assembles already-built artifacts without changing runtime behavior.

**Tech Stack:** C++17, CMake, Fcitx5 headers, Rust CLI, Unix domain socket JSON Lines IPC, SQLite storage adapter, Debian package metadata, strict `-Werror` and `-D warnings` gates.

---

## Scope Boundary

Allowed behavior:

```text
Fcitx5 C++ preedit shim
IPC client handshake tests
Rust CLI privacy and doctor commands
integration gate scripts
Debian package assembly from built artifacts
```

Forbidden behavior:

```text
business logic in C++ shim
Python required-path packaging or CLI code
manual-only privacy deletion
packaging script committed before artifacts it verifies exist
warning suppression in C++ or Rust
```

Rust gates after every Rust task:

```bash
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

C++ gates after every Fcitx5 task:

```bash
cmake -S fcitx5/idiolect-fcitx5 -B fcitx5/idiolect-fcitx5/build -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_CXX_FLAGS="-Wall -Wextra -Wpedantic -Werror"
cmake --build fcitx5/idiolect-fcitx5/build
ctest --test-dir fcitx5/idiolect-fcitx5/build --output-on-failure
```

## Task 1: Fcitx5 C++ Thin Shim Contract

**Owner:** Stronger model or gatekeeper-local for C++ and IME semantics  
**Model:** `gpt-5.4-mini` or gatekeeper-local  
**Files:**

- Create: `fcitx5/idiolect-fcitx5/CMakeLists.txt`
- Create: `fcitx5/idiolect-fcitx5/src/engine.h`
- Create: `fcitx5/idiolect-fcitx5/src/engine.cpp`
- Create: `fcitx5/idiolect-fcitx5/src/ipc_client.h`
- Create: `fcitx5/idiolect-fcitx5/src/ipc_client.cpp`
- Create: `fcitx5/idiolect-fcitx5/tests/preedit_session_test.cpp`
- Create: `ci/scripts/test-fcitx5.sh`

- [x] **Step 1: Write failing C++ preedit tests**

Create a C++ test with `FakeIpcClient`, `Engine::start_recording`, `Engine::receive_preedit`, `Engine::commit_preedit`, and `Engine::visible_preedit`. It must assert IPC messages are `start_recording` then `commit:restart Traefik`, and visible preedit is empty after commit.

- [x] **Step 2: Run red C++ command**

```bash
cmake -S fcitx5/idiolect-fcitx5 -B fcitx5/idiolect-fcitx5/build -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_CXX_FLAGS="-Wall -Wextra -Wpedantic -Werror"
cmake --build fcitx5/idiolect-fcitx5/build
ctest --test-dir fcitx5/idiolect-fcitx5/build --output-on-failure
```

Expected: FAIL because the project and engine classes are absent.

- [x] **Step 3: Implement thin shim classes**

Implement an `IpcClient` abstract interface and `Engine` class. The engine delegates start, cancel, and commit to IPC; it stores visible preedit text; it clears visible preedit on commit and cancel. No ASR, storage, training, privacy, or promotion logic belongs in C++.

- [x] **Step 4: Create C++ gate script**

Create `ci/scripts/test-fcitx5.sh` containing the CMake configure, build, and ctest commands above.

- [x] **Step 5: Run green command and commit**

```bash
bash ci/scripts/test-fcitx5.sh
git add fcitx5/idiolect-fcitx5 ci/scripts/test-fcitx5.sh
git commit -m "feat: add fcitx5 thin preedit shim"
```

## Task 2: IPC Handshake Contract Between Shim And Daemon DTOs

**Owner:** Stronger model or gatekeeper-local for protocol semantics  
**Model:** `gpt-5.4-mini` or gatekeeper-local  
**Files:**

- Modify: `Cargo.lock`
- Modify: `crates/idiolect-ipc/Cargo.toml`
- Modify: `crates/idiolect-ipc/src/lib.rs`
- Modify: `crates/idiolect-ipc/src/messages.rs`
- Modify: `crates/idiolect-ipc/src/framing.rs`
- Modify: `crates/idiolect-ipc/src/handshake.rs`
- Modify: `crates/idiolect-integration-tests/Cargo.toml`
- Modify: `fcitx5/idiolect-fcitx5/src/ipc_client.h`
- Modify: `fcitx5/idiolect-fcitx5/src/ipc_client.cpp`
- Modify: `fcitx5/idiolect-fcitx5/tests/preedit_session_test.cpp`
- Create: `crates/idiolect-integration-tests/tests/ipc_handshake_contract.rs`
- Create: `crates/idiolect-ipc/tests/framing_contract.rs`

- [x] **Step 1: Write failing Rust IPC tests**

Create tests `fcitx5_client_protocol_version_is_accepted` and `unknown_protocol_version_is_rejected`. Version `1` with features `preedit` and `commit` is accepted. Version `99` returns `HandshakeError::UnsupportedProtocolVersion(99)`.

- [x] **Step 2: Run red command**

```bash
cargo test -p idiolect-integration-tests --test ipc_handshake_contract
```

Expected: FAIL because handshake implementation is absent or incomplete.

- [x] **Step 3: Implement versioned IPC DTOs and framing**

Implement JSON Lines framing with message categories `ClientHello`, `ServerHello`, `StartRecording`, `PreeditUpdate`, `CommitPreedit`, `CancelPreedit`, and `Error`. `negotiate_protocol` accepts protocol version `1` only and returns stable feature intersection.

- [x] **Step 4: Run Rust and C++ green commands**

```bash
cargo test -p idiolect-ipc --lib
cargo test -p idiolect-integration-tests --test ipc_handshake_contract
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [x] **Step 5: Commit**

```bash
git add Cargo.lock crates/idiolect-ipc crates/idiolect-integration-tests/Cargo.toml crates/idiolect-integration-tests/tests/ipc_handshake_contract.rs fcitx5/idiolect-fcitx5/src/ipc_client.h fcitx5/idiolect-fcitx5/src/ipc_client.cpp fcitx5/idiolect-fcitx5/tests/preedit_session_test.cpp
git commit -m "feat: add versioned ipc handshake contract"
```

## Task 3: CLI Doctor And Privacy Commands

**Owner:** Gatekeeper-local for privacy semantics, Spark worker for command parsing after semantics accepted  
**Model:** Gatekeeper-local or `gpt-5.4-mini`, then `gpt-5.3-codex-spark`  
**Files:**

- Modify: `crates/idiolect-cli/src/lib.rs`
- Modify: `crates/idiolect-cli/src/main.rs`
- Create: `crates/idiolect-cli/tests/cli_privacy.rs`
- Create: `crates/idiolect-cli/tests/cli_doctor.rs`
- Modify: `crates/idiolect-cli/Cargo.toml`
- Modify: `crates/idiolect-adapter-sqlite/src/repository.rs`
- Create: `crates/idiolect-integration-tests/tests/privacy_delete.rs`

- [ ] **Step 1: Write failing CLI tests**

Create `doctor_command_reports_json_status` using `std::process::Command` and `env!("CARGO_BIN_EXE_idiolect-cli")`; it runs `doctor --json` and asserts stdout contains `"storage"` and `"ipc"`. Create `privacy_delete_requires_explicit_confirm_flag`; it runs `privacy delete --user default`, expects non-zero exit, and asserts stderr contains `--confirm-delete`.

- [ ] **Step 2: Run red command**

```bash
cargo test -p idiolect-cli --tests
```

Expected: FAIL because CLI commands are absent.

- [ ] **Step 3: Implement minimal command parser**

Use `std::env::args` unless a decision record approves an exact pinned CLI parser dependency. Implement:

```text
idiolect-cli doctor --json
idiolect-cli privacy export --user <user> --db <path>
idiolect-cli privacy delete --user <user> --db <path> --confirm-delete
```

Privacy delete must refuse to run without `--confirm-delete` and return a non-zero exit code with a clear stderr message.

- [ ] **Step 4: Add storage privacy deletion integration test**

Create `privacy_delete_removes_user_materialized_data_and_appends_event`: migrate a temp SQLite DB, create and commit one session, call `delete_user_data_for_test("default")`, assert candidate count is `0`, and assert `UserDataDeleted` event count is `1`.

- [ ] **Step 5: Run green commands and gates**

```bash
cargo test -p idiolect-cli --tests
cargo test -p idiolect-integration-tests --test privacy_delete
bash ci/scripts/test-rust.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/idiolect-cli crates/idiolect-adapter-sqlite crates/idiolect-integration-tests/tests/privacy_delete.rs
git commit -m "feat: add cli doctor and privacy commands"
```

## Task 4: Integration Gate Script

**Owner:** Spark worker allowed, gatekeeper reviews command coverage  
**Model:** `gpt-5.3-codex-spark`  
**Files:**

- Create: `ci/scripts/test-integration.sh`

- [ ] **Step 1: Create integration gate script**

Create `ci/scripts/test-integration.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

bash ci/scripts/test-rust.sh
bash ci/scripts/test-fcitx5.sh
cargo test -p idiolect-integration-tests --all-targets --all-features
cargo test -p idiolect-cli --tests
```

- [ ] **Step 2: Run green command**

```bash
bash ci/scripts/test-integration.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 3: Commit**

```bash
git add ci/scripts/test-integration.sh
git commit -m "ci: add integration gate script"
```

## Task 5: Debian Package Assembly

**Owner:** Gatekeeper-local or stronger model for install behavior  
**Model:** Gatekeeper-local or `gpt-5.4-mini`  
**Files:**

- Create: `packaging/debian/DEBIAN/control`
- Create: `packaging/debian/usr/bin/.gitkeep`
- Create: `packaging/debian/usr/lib/fcitx5/.gitkeep`
- Create: `packaging/debian/usr/share/doc/idiolect/README.md`
- Create: `ci/scripts/test-packaging.sh`

- [ ] **Step 1: Verify build artifacts exist before packaging script is committed**

Run:

```bash
cargo build --workspace --release
bash ci/scripts/test-fcitx5.sh
```

Expected: release Rust artifacts and Fcitx5 build output exist locally. If these commands do not pass, do not create the packaging script.

- [ ] **Step 2: Create Debian control metadata**

Create `packaging/debian/DEBIAN/control` with package name `idiolect`, version `0.1.0`, architecture `amd64`, and description `Local-first speech-to-text input method for Linux`.

- [ ] **Step 3: Create packaging gate script**

Create `ci/scripts/test-packaging.sh` that builds release artifacts, runs `test-fcitx5.sh`, copies `idiolect-cli`, `idiolectd`, and `libidiolect-fcitx5.so` into `target/package/idiolect-deb`, builds `target/package/idiolect_0.1.0_amd64.deb` with `dpkg-deb --build`, and verifies package contents include all three artifacts with `dpkg-deb --contents`.

- [ ] **Step 4: Run packaging gate**

```bash
bash ci/scripts/test-packaging.sh
```

Expected: PASS and `target/package/idiolect_0.1.0_amd64.deb` exists.

- [ ] **Step 5: Commit**

```bash
git add packaging/debian ci/scripts/test-packaging.sh
git commit -m "build: add debian packaging gate"
```

## Task 6: Final Release Gate

**Owner:** Gatekeeper-local  
**Model:** Gatekeeper-local  
**Files:**

- Modify: `README.md`

- [ ] **Step 1: Add release gate documentation**

Add a `V1 Verification Gates` section listing:

```bash
bash ci/scripts/test-rust.sh
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-integration.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-packaging.sh
```

The section must state that all warnings are errors and any failing command blocks release.

- [ ] **Step 2: Run all release gates**

```bash
bash ci/scripts/test-rust.sh
bash ci/scripts/test-fcitx5.sh
bash ci/scripts/test-integration.sh
bash ci/scripts/test-real-adapter-deps.sh
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-packaging.sh
```

Expected: PASS with zero warnings.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: document v1 release gates"
```

## Rejection Criteria

Reject and rework this child if any condition holds:

```text
Fcitx5 shim contains ASR, storage, training, promotion, or privacy deletion logic
C++ builds with warnings or without -Werror
CLI privacy delete can run without explicit confirmation
privacy delete removes materialized rows without appending UserDataDeleted event
integration script omits Rust or C++ gates
packaging script is committed before it can build and inspect an artifact
package contents omit daemon, CLI, or Fcitx5 library
any lint, compile, doc, C++ build, C++ test, package, or integration warning appears
```

