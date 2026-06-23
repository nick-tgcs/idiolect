#!/usr/bin/env bash
#
# Build the single UniFFI facade (`idiolect-ffi`) into the artifacts the Android
# Gradle module consumes:
#   1. the per-ABI native libraries (`libidiolect_ffi.so`) laid out as `jniLibs`;
#   2. the generated Kotlin bindings (`uniffi/.../idiolect_ffi.kt`).
#
# This is the M1 deliverable's "produce the real `.so`" half (the housekeeping
# M0 deferred) from docs/future/009-android-implementation-plan.md. The seam
# logic itself is proven host-side by `cargo test -p idiolect-ffi`.
#
# whisper.cpp links libc++_shared.so dynamically, so the APK must also bundle the
# NDK's libc++_shared.so per ABI — copied alongside here.
#
# Usage: scripts/android-ffi-build.sh [out_dir] [extra cargo args]
#   default out_dir: target/android/idiolect-ffi
#   ANDROID_NDK_HOME=/path/to/ndk scripts/android-ffi-build.sh --release
set -euo pipefail

# --- Locate the NDK (all four vars; cmake needs more than ANDROID_NDK_HOME) ---
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

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="${1:-$repo_root/target/android/idiolect-ffi}"
[[ $# -gt 0 ]] && shift || true
api="${ANDROID_API:-31}"

jni_dir="$out_dir/jniLibs"
kotlin_dir="$out_dir/kotlin"
mkdir -p "$jni_dir" "$kotlin_dir"

# --- 1. Cross-build the cdylib into a jniLibs layout -----------------------
# ABIs come from ANDROID_ABIS (space- or comma-separated); default both the device
# (arm64-v8a) and the emulator (x86_64). The release workflow sets ANDROID_ABIS=arm64-v8a.
IFS=', ' read -r -a abis <<< "${ANDROID_ABIS:-arm64-v8a x86_64}"
target_flags=()
for abi in "${abis[@]}"; do target_flags+=(-t "$abi"); done
echo "Building libidiolect_ffi.so for ${abis[*]} (API $api)..."
cargo ndk "${target_flags[@]}" --platform "$api" -o "$jni_dir" \
  build -p idiolect-ffi "$@"

# --- 2. Bundle the NDK's libc++_shared.so per ABI -------------------------
# whisper.cpp links it dynamically; without it the app crashes at load.
declare -A abi_triple=( [arm64-v8a]=aarch64-linux-android [x86_64]=x86_64-linux-android )
sysroot_lib="$ndk/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib"
for abi in "${abis[@]}"; do
  triple="${abi_triple[$abi]:-}"
  [[ -n "$triple" ]] || { echo "warn: no known triple for ABI $abi; skipping libc++_shared.so" >&2; continue; }
  src="$sysroot_lib/$triple/libc++_shared.so"
  if [[ -f "$src" && -d "$jni_dir/$abi" ]]; then
    cp "$src" "$jni_dir/$abi/libc++_shared.so"
    echo "bundled libc++_shared.so for $abi"
  fi
done

# --- 3. Generate the Kotlin bindings from the built library ----------------
# Generate from whichever ABI built; the bindings are ABI-independent.
profile_dir="debug"
for a in "$@"; do [[ "$a" == "--release" ]] && profile_dir="release"; done
lib=""
for triple in aarch64-linux-android x86_64-linux-android; do
  cand="$repo_root/target/$triple/$profile_dir/libidiolect_ffi.so"
  [[ -f "$cand" ]] && lib="$cand" && break
done
if [[ -z "$lib" ]]; then
  echo "error: built libidiolect_ffi.so not found for binding generation" >&2
  exit 1
fi
echo "Generating Kotlin bindings from $lib ..."
cargo run -q -p idiolect-ffi --bin uniffi-bindgen -- \
  generate --library "$lib" --language kotlin --out-dir "$kotlin_dir" --no-format

echo "OK: idiolect-ffi built."
echo "  jniLibs:  $jni_dir"
echo "  bindings: $kotlin_dir"
