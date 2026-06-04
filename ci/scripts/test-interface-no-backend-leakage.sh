#!/usr/bin/env bash
set -euo pipefail

if rg -n "\bcpal\b|\bwhisper\b|\bsilero\b|fast-vad|webrtc-vad|\bwebrtc\b|\bfvad\b|\blibfvad\b|\bopus\b|\bonnx\b|\bort\b|\brusqlite\b|\bpytorch\b|\bpeft\b|\bpython\b" \
  crates/idiolect-core crates/idiolect-ports crates/idiolect-application; then
  echo "backend implementation detail leaked into interface crates" >&2
  exit 1
fi
