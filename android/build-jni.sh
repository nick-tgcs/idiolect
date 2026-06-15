#!/usr/bin/env bash
# Cross-compile the idiolect-ffi cdylib for each requested Android ABI and stage it
# into the app's jniLibs, so the APK ships the native core (whisper.cpp/ggml + opus +
# sqlite, CPU-only). Each ABI is built in its own invocation because the cmake-android
# configuration (CMAKE_ANDROID_ARCH_ABI) is process-global.
#
# Usage: build-jni.sh ["<abi> <abi> ..."] [debug|release]
#   defaults: "x86_64 arm64-v8a" release   (x86_64 = emulator, arm64-v8a = device)
set -euo pipefail

ABIS="${1:-x86_64 arm64-v8a}"
PROFILE="${2:-release}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$SCRIPT_DIR/.." && pwd)"
JNILIBS="$SCRIPT_DIR/app/src/main/jniLibs"

: "${ANDROID_NDK_HOME:=${ANDROID_HOME:-$HOME/Android/Sdk}/ndk/28.0.13004108}"
export ANDROID_NDK_HOME
# whisper.cpp builds via cmake; point cmake's integrated Android support at the NDK.
# (whisper-rs-sys forwards any CMAKE_*-prefixed env var into the cmake config.)
export CMAKE_ANDROID_NDK="$ANDROID_NDK_HOME"
export CMAKE_SYSTEM_VERSION=21

profile_flag=()
[ "$PROFILE" = "release" ] && profile_flag=("--release")

for abi in $ABIS; do
  echo ">>> building idiolect-ffi for $abi ($PROFILE)"
  CMAKE_ANDROID_ARCH_ABI="$abi" \
    cargo ndk -t "$abi" -o "$JNILIBS" "${profile_flag[@]}" \
    build -p idiolect-ffi --manifest-path "$WORKSPACE/Cargo.toml"
done

echo ">>> staged:"
find "$JNILIBS" -name 'libidiolect_ffi.so' -printf '    %p (%s bytes)\n'
