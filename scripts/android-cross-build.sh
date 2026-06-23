#!/usr/bin/env bash
#
# Cross-compile the portable Idiolect core (the "brain": ASR, codec, storage,
# VAD, application, and the sync crates) to the Android ABIs.
#
# This proves the native stack builds for real devices (arm64-v8a) and for the
# emulator (x86_64). It is the M0 build gate from
# docs/future/009-android-implementation-plan.md — the daemon/desktop-only crates
# (idiolectd, tray, dialogs, CLI) are deliberately excluded; only the crates the
# Android runtime will reuse are built here.
#
# Requirements: the Android NDK (r28 tested) and the aarch64/x86_64 Android rust
# targets + cargo-ndk:
#   rustup target add aarch64-linux-android x86_64-linux-android
#   cargo install cargo-ndk --locked
#
# Usage: scripts/android-cross-build.sh [extra cargo args]
#   ANDROID_NDK_HOME=/path/to/ndk scripts/android-cross-build.sh --release
set -euo pipefail

# --- Locate the NDK -------------------------------------------------------
# The `cmake` crate (used by whisper.cpp and opus) only finds the NDK when one
# of ANDROID_NDK_ROOT / ANDROID_NDK / NDK_HOME is set — ANDROID_NDK_HOME alone is
# NOT enough. We export all of them from one resolved path.
ndk="${ANDROID_NDK_HOME:-}"
if [[ -z "$ndk" ]]; then
  for base in "${ANDROID_SDK_ROOT:-}" "${ANDROID_HOME:-}" "$HOME/Android/Sdk" "/usr/lib/android-sdk"; do
    [[ -n "$base" && -d "$base/ndk" ]] || continue
    latest="$(ls -1 "$base/ndk" | sort -V | sed -n '$p')"
    [[ -n "$latest" ]] && ndk="$base/ndk/$latest" && break
  done
fi
if [[ -z "$ndk" || ! -d "$ndk" ]]; then
  echo "error: Android NDK not found. Install it or set ANDROID_NDK_HOME." >&2
  exit 1
fi
export ANDROID_NDK_HOME="$ndk" ANDROID_NDK_ROOT="$ndk" ANDROID_NDK="$ndk" NDK_HOME="$ndk"
echo "Using Android NDK: $ndk"

# --- Build ----------------------------------------------------------------
api="${ANDROID_API:-31}"   # min OS = Android 12 / API 31

# The portable core: everything the Android runtime reuses. Keep in sync with
# the workspace; desktop-only crates (idiolectd, ksni, dialogs, cli) stay out.
core_crates=(
  idiolect-common
  idiolect-core
  idiolect-ports
  idiolect-application
  idiolect-adapter-sqlite
  idiolect-adapter-opus
  idiolect-adapter-vad
  idiolect-adapter-whisper
  idiolect-sync
  idiolect-sync-client
  idiolect-sync-server
  idiolect-ffi
)

pkg_args=()
for crate in "${core_crates[@]}"; do pkg_args+=(-p "$crate"); done

echo "Building core for arm64-v8a + x86_64 (API $api)..."
cargo ndk -t arm64-v8a -t x86_64 --platform "$api" build "${pkg_args[@]}" "$@"
echo "OK: portable core cross-compiled for arm64-v8a and x86_64."
