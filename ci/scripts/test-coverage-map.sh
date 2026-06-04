#!/usr/bin/env bash
set -euo pipefail

coverage_map="docs/quality/v1-coverage-map.md"
required_processes=(
  "audio.capture"
  "audio.fixture"
  "codec.opus"
  "vad.segment"
  "asr.whisper"
  "daemon.startup"
  "ipc.handshake"
  "ipc.lifecycle"
  "fcitx5.preedit"
  "fcitx5.commit"
  "fcitx5.cancel"
  "storage.event_log"
  "storage.materialized_tables"
  "candidate.capture"
  "learning.classifier"
  "learning.manifest"
  "learning.promotion"
  "learning.rollback"
  "privacy.export"
  "privacy.delete"
  "privacy.deleted_data_excluded"
  "package.payload"
  "package.smoke"
)

if [[ ! -f "$coverage_map" ]]; then
  echo "coverage map missing: $coverage_map" >&2
  exit 1
fi

if rg -q "UNASSIGNED" "$coverage_map"; then
  echo "coverage map contains UNASSIGNED rows" >&2
  exit 1
fi

for process in "${required_processes[@]}"; do
  if ! rg -q "^\|[[:space:]]*${process}[[:space:]]*\|" "$coverage_map"; then
    echo "coverage map missing required process: $process" >&2
    exit 1
  fi
done
