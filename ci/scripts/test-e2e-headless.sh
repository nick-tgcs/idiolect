#!/usr/bin/env bash
set -euo pipefail

if [[ "${IDIOLECT_HEADLESS_DESKTOP_READY:-}" != "1" ]]; then
  # The headless desktop app-matrix needs a prepared X11/Wayland + Fcitx5 target
  # that a standard CI runner doesn't provide. Skip (pass) here rather than fail
  # the gate; set IDIOLECT_HEADLESS_DESKTOP_READY=1 on a prepared runner to run it.
  echo "skipping headless desktop app-matrix: set IDIOLECT_HEADLESS_DESKTOP_READY=1 with a prepared X11/Wayland+Fcitx5 target to run it"
  exit 0
fi

for tool in fcitx5 fcitx5-remote; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "required headless desktop tool missing: ${tool}" >&2
    exit 1
  fi
done

bash ci/scripts/test-fcitx5-integration.sh
printf '%s\n' \
  "manual target app matrix still required: Firefox" \
  "manual target app matrix still required: Chromium" \
  "manual target app matrix still required: terminal" \
  "manual target app matrix still required: GTK editor" \
  "manual target app matrix still required: Qt editor" \
  "manual target app matrix still required: VS Code/Electron" >&2
exit 1
