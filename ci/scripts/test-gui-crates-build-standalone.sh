#!/usr/bin/env bash
# Each eframe/winit GUI binary is spawned as its OWN process by the daemon
# (e.g. the engine launches `idiolect-review-dialog` beside itself). A full
# `cargo build --workspace` UNIFIES Cargo features across every member, so if
# one GUI crate forgets the platform windowing backend (eframe's `x11`/
# `wayland`) but a SIBLING requests it, the workspace build still links a
# backend in and the binary looks fine — while an isolated build of that one
# crate (packaging, `cargo install --path`, a trimmed release) produces a
# binary that cannot open a window and dies at launch ("nothing shows").
#
# This gate builds every GUI binary crate on its own, defeating that
# unification, so a missing windowing backend fails CI instead of the user's
# desktop. Regression guard for the eframe 0.34 migration, which shipped
# review-dialog / retention-dialog / settings without `x11`.
set -euo pipefail

cd "$(dirname "$0")/../.."

gui_crates=(
  idiolect-review-dialog
  idiolect-retention-dialog
  idiolect-recording-indicator
  idiolect-settings
  idiolect-app
)

status=0
for crate in "${gui_crates[@]}"; do
  echo "== building ${crate} in isolation =="
  if ! cargo build -p "${crate}" --quiet; then
    echo "GUI crate ${crate} does not build standalone — a required eframe" >&2
    echo "windowing backend (x11/wayland) is likely missing from its Cargo.toml." >&2
    status=1
  fi
done

exit "${status}"
