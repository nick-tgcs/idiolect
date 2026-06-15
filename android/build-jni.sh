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

# whisper.cpp links the NDK's shared libc++ (cargo:rustc-link-lib=dylib=c++_shared),
# so libidiolect_ffi.so NEEDs libc++_shared.so at runtime — it must ship alongside.
NDK_SYSROOT_LIB="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib"
abi_triple() {
  case "$1" in
    x86_64) echo x86_64-linux-android ;;
    arm64-v8a) echo aarch64-linux-android ;;
    armeabi-v7a) echo arm-linux-androideabi ;;
    x86) echo i686-linux-android ;;
    *) echo "unknown abi: $1" >&2; return 1 ;;
  esac
}

for abi in $ABIS; do
  echo ">>> building idiolect-ffi for $abi ($PROFILE)"
  CMAKE_ANDROID_ARCH_ABI="$abi" \
    cargo ndk -t "$abi" -o "$JNILIBS" "${profile_flag[@]}" \
    build -p idiolect-ffi --manifest-path "$WORKSPACE/Cargo.toml"
  # Stage the matching libc++_shared.so next to it.
  triple="$(abi_triple "$abi")"
  cp -f "$NDK_SYSROOT_LIB/$triple/libc++_shared.so" "$JNILIBS/$abi/libc++_shared.so"
done

echo ">>> staged:"
find "$JNILIBS" -name 'libidiolect_ffi.so' -printf '    %p (%s bytes)\n'
