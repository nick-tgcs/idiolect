#!/usr/bin/env bash
# Runs the IBus engine end-to-end tests. The history-Insert e2e drives the real
# `ibus-engine-idiolect` binary against a fake-daemon socket over a PRIVATE
# `dbus-daemon` that the test spawns itself — so it is a normal `#[test]` that
# needs no ambient desktop, no `dbus-run-session` wrapper, and no `--ignored`.
#
# It does need the `dbus-daemon` binary on PATH (the `dbus` package). The
# full-dictation e2e (`engine_dictates_…`) stays `#[ignore]` — it spawns the real
# daemon + KSNI tray, which a bare private bus cannot host — so it is not run here.
set -euo pipefail

if ! command -v dbus-daemon >/dev/null 2>&1; then
  echo "dbus-daemon not found (install the 'dbus' package)" >&2
  exit 1
fi

cargo test -p idiolect-ibus --features ibus-engine --test ibus_engine_e2e
