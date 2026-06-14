#!/usr/bin/env bash
#
# Run the portable Idiolect core's tests ON a connected Android device/emulator
# (x86_64), proving the cross-built native stack actually *executes* — the M0
# "run half" from docs/future/009-android-implementation-plan.md.
#
# It pushes the whisper fixture model and the NDK's libc++_shared.so to the
# device, then runs each device-runnable crate's tests via cargo-ndk's runner.
# whisper links libc++_shared dynamically, so we bake /data/local/tmp into the
# test binaries' rpath and push the runtime there.
#
# Prereqs: a booted emulator (x86_64) reachable over adb, plus the toolchain from
# android-cross-build.sh. Boot one with, e.g.:
#   $ANDROID_SDK_ROOT/emulator/emulator -avd <name> -no-window -no-audio -gpu swiftshader_indirect
set -euo pipefail

# --- locate NDK + SDK -----------------------------------------------------
ndk="${ANDROID_NDK_HOME:-}"
if [[ -z "$ndk" ]]; then
  for base in "${ANDROID_SDK_ROOT:-}" "${ANDROID_HOME:-}" "$HOME/Android/Sdk" "/usr/lib/android-sdk"; do
    [[ -n "$base" && -d "$base/ndk" ]] || continue
    latest="$(ls -1 "$base/ndk" | sort -V | sed -n '$p')"
    [[ -n "$latest" ]] && ndk="$base/ndk/$latest" && break
  done
fi
[[ -n "$ndk" && -d "$ndk" ]] || { echo "error: NDK not found; set ANDROID_NDK_HOME" >&2; exit 1; }
export ANDROID_NDK_HOME="$ndk" ANDROID_NDK_ROOT="$ndk" ANDROID_NDK="$ndk" NDK_HOME="$ndk"

sdk="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Android/Sdk}}"
export ANDROID_SDK_ROOT="$sdk" ANDROID_HOME="$sdk"
adb="$sdk/platform-tools/adb"
export PATH="$sdk/platform-tools:$PATH"

api="${ANDROID_API:-31}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# --- require a device -----------------------------------------------------
if ! "$adb" get-state >/dev/null 2>&1; then
  echo "error: no Android device/emulator over adb. Boot an x86_64 emulator first." >&2
  exit 1
fi
echo "Device: $("$adb" shell getprop ro.product.cpu.abi) API $("$adb" shell getprop ro.build.version.sdk)"

# --- push native runtime + whisper fixture model --------------------------
libcxx="$ndk/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/x86_64-linux-android/libc++_shared.so"
"$adb" shell mkdir -p /data/local/tmp/whisper
"$adb" push "$libcxx" /data/local/tmp/libc++_shared.so >/dev/null
model="$repo_root/tests/fixtures/whisper/ggml-tiny.en.bin"
if [[ -f "$model" ]]; then
  "$adb" push "$model" /data/local/tmp/whisper/ggml-tiny.en.bin >/dev/null
  echo "pushed whisper fixture model"
else
  echo "warning: whisper fixture model missing ($model); whisper test will be skipped/fail" >&2
fi

# --- run device-runnable crate tests --------------------------------------
# Only crates whose tests don't need the desktop daemon (Unix sockets/tray).
device_crates=(
  idiolect-common
  idiolect-application
  idiolect-adapter-opus
  idiolect-adapter-vad
  idiolect-adapter-sqlite
  idiolect-adapter-whisper
  idiolect-sync
)
pkg_args=()
for crate in "${device_crates[@]}"; do pkg_args+=(-p "$crate"); done

# rpath so the dynamically-linked libc++_shared.so (whisper) resolves on device.
export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,/data/local/tmp"
echo "Running core tests on device..."
cargo ndk -t x86_64 --platform "$api" test "${pkg_args[@]}" "$@"
echo "OK: portable core (incl. whisper decode) passes on the Android emulator."
