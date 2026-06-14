# CI/CD Pipeline Documentation

This document describes the GitHub Actions workflows for the Idiolect project.

## Workflow Overview

### 1. `ci.yml` - Main CI Pipeline (Runs on push to main and PRs)
**Comprehensive test suite that must pass before merging.**

Jobs (run in parallel where possible):
- **rust**: Format, clippy, check, test, doc tests, coverage (80% line threshold)
- **fcitx5**: CMake configure, build, ctest
- **integration**: Integration tests (needs rust + fcitx5)
- **e2e**: End-to-end tests with fixture and real adapters (needs rust + fcitx5)
- **failure-recovery**: Daemon lifecycle and error handling tests (needs rust)
- **model-regression**: Whisper model regression tests (needs rust)
- **performance**: Startup/transcription latency and memory benchmarks (needs rust)
- **real-adapter-deps**: Dependency version and Python crate checks (needs rust)
- **interface-leakage**: Architecture boundary enforcement (needs rust)
- **packaging**: Debian package build, smoke, lifecycle tests (needs rust + fcitx5)
- **coverage-map**: Validates docs/quality/v1-coverage-map.md and v1-acceptance-evidence.md (needs all above)
- **e2e-headless**: Headless GUI tests for manual-required acceptance items (needs rust + fcitx5)
- **gate**: Final summary job requiring all above to pass

### 2. `pr-validation.yml` - PR Validation (Runs on PR open/sync)
**Fast feedback for pull requests.**

Jobs:
- **pr-size**: Warns if PR > 50 files or > 1000 lines
- **conventional-commits**: Enforces conventional commit message format
- **rust-quick**: fmt, check, clippy, test (no doc tests, no coverage)
- **fcitx5-quick**: CMake, build, ctest
- **interface-leakage**: Architecture boundary check
- **real-adapter-deps**: Dependency version check
- **coverage-map**: Documentation sync validation (needs rust-quick + fcitx5-quick)
- **pr-validation-summary**: Final gate

### 3. `release.yml` - Stable Release Pipeline (version tags `v*`)
**Builds and publishes stable, versioned release artifacts.**

Jobs:
- **full-ci**: Reuses `ci.yml` as a safety gate (must pass)
- **build-release**: Builds release binaries and the Debian package
- **create-release**: Creates the versioned GitHub Release with artifacts
  (`idiolectd`, `idiolect-cli`, `idiolect-trainerctl`, and the `.deb` — there is
  no bare `idiolect` binary; the CLI ships as `idiolect-cli`)

### 3a. `release-main.yml` - Rolling Edge Release (every push to `main`)
**Always keeps a downloadable build of `main` HEAD.**

Runs on every push to `main` (and via manual `workflow_dispatch`). Reuses
`ci.yml` as a gate (never ships a red build), then publishes a single rolling
`edge` **prerelease**, moving the `edge` tag to the latest green commit. Stable,
versioned releases still come only from `v*` tags via `release.yml`.

Jobs: **full-ci** (reuses `ci.yml`) → **build-release** → **publish-edge**.

> Both release workflows call `ci.yml` as a reusable workflow, which is why
> `ci.yml` declares a `workflow_call` trigger.

### 4. `scheduled.yml` - Scheduled Checks
**Nightly and weekly comprehensive runs.**

- **Nightly (2 AM UTC)**: Full test suite with nightly Rust, 85% coverage, security audit, dependency checks
- **Weekly (Sunday 3 AM UTC)**: Full test suite + cargo-machete (unused deps), cargo-deny (licenses), SBOM generation
- **Manual dispatch**: On-demand full test suite

### 5. `dependabot.yml` - Dependency Updates
**Automated dependency management.**

- Weekly Rust dependency updates (grouped by minor/patch vs major)
- Weekly GitHub Actions updates

## Local Development

### Run Full CI Locally
```bash
# Run all CI checks (same as ci.yml)
bash ci/scripts/test-all.sh

# Run coverage map validation
bash ci/scripts/test-coverage-map.sh
```

### Run PR Validation Locally
```bash
# Quick validation (same as pr-validation.yml rust-quick)
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features

# FCITX5 quick validation
cmake -S fcitx5/idiolect-fcitx5 -B fcitx5/idiolect-fcitx5/build \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DCMAKE_CXX_FLAGS="-Wall -Wextra -Wpedantic -Werror"
cmake --build fcitx5/idiolect-fcitx5/build
ctest --test-dir fcitx5/idiolect-fcitx5/build --output-on-failure

# Architecture checks
bash ci/scripts/test-interface-no-backend-leakage.sh
bash ci/scripts/test-real-adapter-deps.sh

# Coverage map validation
bash ci/scripts/test-coverage-map.sh
```

### Run Individual Test Suites
```bash
# Rust only
bash ci/scripts/test-rust.sh

# FCITX5 only
bash ci/scripts/test-fcitx5.sh

# Integration tests
bash ci/scripts/test-integration.sh

# E2E tests
bash ci/scripts/test-e2e.sh

# Failure recovery
bash ci/scripts/test-e2e-failure-recovery.sh

# Model regression (requires fixture model)
bash ci/scripts/fetch-whisper-fixture.sh
bash ci/scripts/test-model-regression.sh

# Performance
bash ci/scripts/test-performance.sh

# Packaging
bash ci/scripts/test-packaging.sh
bash ci/scripts/test-package-smoke.sh
bash ci/scripts/test-package-lifecycle.sh
```

## Required Tools

### For Local Development
```bash
# Rust toolchain (version pinned in rust-toolchain.toml)
rustup install 1.96.0
rustup component add rustfmt clippy

# Coverage tool
cargo install cargo-llvm-cov

# System dependencies (Ubuntu/Debian)
sudo apt-get install -y \
  cmake g++ \
  libfcitx5-dev libfcitx5utils-dev libfcitx5config-dev \
  libfcitx5qt-dev libfcitx5qt1-dev qtbase5-dev libglib2.0-dev \
  libasound2-dev libpulse-dev \
  dpkg-dev time

# Optional: for scheduled checks
cargo install cargo-audit cargo-outdated cargo-machete cargo-deny cargo-cyclonedx
```

## Coverage Requirements

- **CI (ci.yml)**: 80% line coverage minimum (`--fail-under-lines 80`)
- **Nightly (scheduled.yml)**: 85% line coverage minimum
- **PR Validation**: No coverage gate (fast feedback)

## Architecture Enforcement

The following checks enforce architectural boundaries:

1. **Interface No Backend Leakage** (`test-interface-no-backend-leakage.sh`):
   - Scans core crates for backend implementation types (cpal, whisper, silero, opus, rusqlite, etc.)
   - Fails if any backend type appears in `idiolect-core`, `idiolect-ports`, or `idiolect-application`

2. **Real Adapter Dependency Check** (`test-real-adapter-deps.sh`):
   - Ensures all Cargo.toml version requirements are pinned (no `*`, `^`, `~`)
   - Blocks Python-related crates (numpy, pyo3, etc.) from required dependency paths

3. **Coverage Map Validation** (`test-coverage-map.sh`):
   - Validates `docs/quality/v1-coverage-map.md` has all required processes with valid automated tests
   - Validates `docs/quality/v1-acceptance-evidence.md` has all required acceptance IDs
   - Ensures no suppressed/ignored tests or lints in Rust or C++ code
   - Ensures coverage gate (`ci/scripts/test-all.sh`) invokes all required scripts

## Acceptance Evidence

The project tracks acceptance criteria in two documents:

1. **v1-coverage-map.md**: Maps each process to an automated test
2. **v1-acceptance-evidence.md**: Maps each acceptance criterion to a test command and status (automated vs manual-required)

These are validated by `test-coverage-map.sh` in CI.

## PR Requirements

Before merging, PRs must:
1. Pass all `pr-validation.yml` checks
2. Pass all `ci.yml` checks (triggered on push to main)
3. Have conventional commit messages
4. Be reasonably sized (< 50 files, < 1000 lines changed)
5. Have no suppressed tests or lints
6. Keep coverage map and acceptance evidence in sync

## Release Process

1. Create and push a version tag: `git tag v0.1.0 && git push origin v0.1.0`
2. `release.yml` workflow runs:
   - Full CI must pass
   - Builds release binaries and Debian package
   - Creates GitHub Release with artifacts and auto-generated notes

## Troubleshooting

### CI Fails on Coverage
```bash
# Check coverage locally
cargo llvm-cov --workspace --all-features --all-targets --fail-under-lines 80

# Generate HTML report for inspection
cargo llvm-cov --workspace --all-features --all-targets --html
# Open target/llvm-cov/html/index.html
```

### CI Fails on Interface Leakage
```bash
# Check what's leaking
bash ci/scripts/test-interface-no-backend-leakage.sh
# Output shows file:line matches
```

### CI Fails on Coverage Map
```bash
# Run validation with verbose output
bash ci/scripts/test-coverage-map.sh
# Check docs/quality/v1-coverage-map.md and v1-acceptance-evidence.md
```

### FCITX5 Build Fails
```bash
# Ensure all dependencies installed
sudo apt-get install -y cmake g++ libfcitx5-dev libfcitx5utils-dev libfcitx5config-dev libfcitx5qt-dev libfcitx5qt1-dev qtbase5-dev libglib2.0-dev

# Clean rebuild
rm -rf fcitx5/idiolect-fcitx5/build
cmake -S fcitx5/idiolect-fcitx5 -B fcitx5/idiolect-fcitx5/build -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_CXX_FLAGS="-Wall -Wextra -Wpedantic -Werror"
cmake --build fcitx5/idiolect-fcitx5/build
ctest --test-dir fcitx5/idiolect-fcitx5/build --output-on-failure
```