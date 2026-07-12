#!/usr/bin/env bash
# Hermetic-build gate: the whisper.cpp (ggml) build must not depend on the build
# machine's CPU. ggml defaults to GGML_NATIVE=ON (-march=native), so a binary built
# on an AVX-512 runner SIGILLs on an AVX2-only one. CI shares target/ caches across
# GitHub's mixed-ISA runner fleet and the release workflows ship these binaries, so
# the build must target the portable baseline everywhere (PR #67 whisper SIGILL).
#
# Test levels: CI/build configuration has no unit or e2e seam — the only reachable
# level is this integration gate over the repo config, the workflows, and the
# compiled artifact itself.
set -euo pipefail
cd "$(dirname "$0")/../.."

fail=0

# 1. The repo-wide pin: every cargo invocation (all workflows, local builds) must
#    default whisper-rs-sys to a portable ggml build.
if ! grep -qE '^GGML_NATIVE *= *"OFF"' .cargo/config.toml; then
  echo "FAIL: .cargo/config.toml does not pin GGML_NATIVE = \"OFF\" under [env]" >&2
  fail=1
fi

# 2. Cache hygiene: cargo caches predating the pin hold machine-specific objects, and
#    cargo cannot re-fingerprint them (whisper-rs-sys declares no rerun-if-env-changed
#    for GGML_*). Every cargo cache reference must therefore live in the -portable-
#    namespace (old entries unreachable), and every key must hash .cargo/config.toml
#    so a config change can never exact-hit a stale cache.
while IFS= read -r hit; do
  echo "FAIL: cargo cache reference outside the -portable- namespace: $hit" >&2
  fail=1
done < <(grep -rn -- '}}-cargo-' .github/workflows/ | grep -v -- '-cargo-portable-')

while IFS= read -r hit; do
  echo "FAIL: cargo cache key does not hash .cargo/config.toml: $hit" >&2
  fail=1
done < <(grep -rn 'key:.*-cargo-portable-.*hashFiles' .github/workflows/ | grep -v '\.cargo/config\.toml')

# 3. The artifact itself: the whisper test binary CI just built must be portable.
#    ggml's AVX-512 kernels use 512-bit zmm registers, and -march=native on newer
#    fleets also emits EVEX/VNNI forms (vpternlog*, vpdpbusd) — none may be present.
#    This catches any regression path the config checks can't see (e.g. a stale
#    cache reused past a config change).
if [[ "$(uname -sm)" == "Linux x86_64" ]]; then
  binary=$(cargo test -p idiolect-adapter-whisper --no-run --message-format=json 2>/dev/null |
    jq -r 'select(.reason == "compiler-artifact" and .executable != null)
           | select(.manifest_path | contains("idiolect-adapter-whisper")) | .executable' |
    tail -n 1)
  if [[ -z "$binary" ]]; then
    echo "FAIL: could not locate the idiolect-adapter-whisper test binary" >&2
    fail=1
  else
    # grep -c (not -q): -q would close the pipe on first match and objdump's
    # SIGPIPE, under pipefail, would silently falsify the condition.
    evex_count=$(objdump -d "$binary" | { grep -cE '%zmm|vpternlog|vpdpbusd' || true; })
    if [[ "$evex_count" -gt 0 ]]; then
      echo "FAIL: $binary contains $evex_count AVX-512/EVEX instructions — the ggml build is not portable" >&2
      fail=1
    fi
  fi
fi

if [[ "$fail" -ne 0 ]]; then
  echo "hermetic-build gate: FAILED" >&2
  exit 1
fi
echo "hermetic-build gate: OK"
