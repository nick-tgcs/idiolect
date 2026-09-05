#!/usr/bin/env bash
set -euo pipefail

# Without this, a missing `rg` is INVISIBLE: every check below is written as
# `if rg ...; then fail; fi`, so the 127 from a command that is not there is
# read as "no matches" and the gate reports a clean pass having examined
# nothing. That is not hypothetical — the ubuntu-24.04 runner image does not
# ship ripgrep, and this script printed `rg: command not found` and exited 0
# in CI.
if ! command -v rg >/dev/null 2>&1; then
  echo "ripgrep (rg) is required by this check but is not installed" >&2
  exit 1
fi

if rg -n "\bcpal\b|\bwhisper\b|\bsilero\b|fast-vad|webrtc-vad|\bwebrtc\b|\bfvad\b|\blibfvad\b|\bopus\b|\bonnx\b|\bort\b|\brusqlite\b|\bpytorch\b|\bpeft\b|\bpython\b" \
  crates/idiolect-core crates/idiolect-ports crates/idiolect-application; then
  echo "backend implementation detail leaked into interface crates" >&2
  exit 1
fi
