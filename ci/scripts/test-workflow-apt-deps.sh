#!/usr/bin/env bash
# Tests for check-workflow-apt-deps.sh — the workflow apt-package gate.
#
# The gate exists because a wrong package name in a workflow is only ever proved
# wrong by running that workflow, and release-main.yml runs a few times a year.
# It failed all three times it has ever run, on `libfcitx5-dev`, and its release
# job never executed once as a result. A gate for that has to be right about two
# things in opposite directions: it must catch a name that does not exist, and
# it must not invent one out of a shell operator or a `${{ }}` expression, since
# a false red here blocks every PR.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$SCRIPT_DIR/check-workflow-apt-deps.sh"

PASSED=0
FAILED=0

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    FAILED=$((FAILED + 1))
}
ok() {
    printf 'ok: %s\n' "$1"
    PASSED=$((PASSED + 1))
}

# Without this every negative case below would pass on exit 127 — the subject
# missing looks exactly like the subject rejecting bad input.
if [ ! -x "$CHECK" ]; then
    printf 'FAIL: %s is missing or not executable — no case below would mean anything\n' "$CHECK" >&2
    exit 1
fi

if ! command -v apt-cache >/dev/null 2>&1; then
    printf 'FAIL: apt-cache not found — these cases cannot distinguish a real package from a fake one\n' >&2
    exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# `write_workflow <dir-name> <file-name>` reading the body from stdin.
write_workflow() {
    mkdir -p "$WORK/$1"
    cat >"$WORK/$1/$2"
}

run_check() { # run_check "<path> [path ...]" -> exit code, output on stdout
    local args=() path
    for path in $1; do
        args+=("$WORK/$path")
    done
    "$CHECK" "${args[@]}" 2>&1
}

expect() { # expect <label> <want-exit> "<path> [path ...]" [must-contain ...]
    local label="$1" want="$2" paths="$3"
    shift 3
    local out got
    out="$(run_check "$paths")"
    got=$?

    if [ "$got" -ne "$want" ]; then
        fail "$label: expected exit $want, got $got"
        printf '%s\n' "$out" | sed 's/^/    | /' >&2
        return
    fi
    local needle
    for needle in "$@"; do
        if ! printf '%s' "$out" | grep -qF -- "$needle"; then
            fail "$label: output does not mention '$needle'"
            printf '%s\n' "$out" | sed 's/^/    | /' >&2
            return
        fi
    done
    ok "$label"
}

# ---------------------------------------------------------------- the real bug
# The line release-main.yml actually carried, verbatim. If this case ever goes
# green the gate has stopped catching the defect it was written for.
write_workflow real release-main.yml <<'YAML'
      - name: Install system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y cmake g++ libfcitx5-dev libfcitx5utils-dev libfcitx5config-dev libfcitx5qt-dev libfcitx5qt1-dev qtbase5-dev libglib2.0-dev dpkg-dev
YAML
expect "the historical release-main.yml line is rejected" 1 real \
    "libfcitx5-dev" "libfcitx5qt-dev" "libfcitx5qt1-dev" "release-main.yml:4"

# The corrected list, taken from the CI job that passes. This is the other half
# of the same case: the fix has to actually satisfy the gate.
write_workflow fixed release-main.yml <<'YAML'
      - name: Install system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y cmake g++ libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev libglib2.0-dev libasound2-dev libpulse-dev dpkg-dev
YAML
expect "the corrected package list is accepted" 0 fixed \
    "All 9 apt package(s)"

# ------------------------------------------------------- no inventing packages
# `&&` ends the package list. Without the operator check, `echo` and `installed`
# are looked up as packages, do not resolve, and the gate reports a defect in a
# correct workflow — the failure mode that makes a gate get switched off.
write_workflow operators ci.yml <<'YAML'
        run: sudo apt-get install -y cmake && echo installed
YAML
expect "a shell operator ends the package list" 0 operators \
    "All 1 apt package(s)"

# ...but it ends only THAT list. A second install command on the same line is
# still a second install command, and dropping it is silent under-coverage,
# which reads exactly like a clean result.
write_workflow chained ci.yml <<'YAML'
        run: sudo apt-get install -y cmake && sudo apt-get install -y libfcitx5-dev
YAML
expect "a second install command on the same line is checked" 1 chained \
    "libfcitx5-dev"

# A name assembled at runtime cannot be resolved here. It must be announced and
# skipped, not guessed at in either direction.
write_workflow interpolated ci.yml <<'YAML'
        run: sudo apt-get install -y cmake ${{ matrix.extra_package }} $EXTRA
YAML
expect "an interpolated package name is announced, not judged" 0 interpolated \
    "not checked: \${{matrix.extra_package}}" "not checked: \$EXTRA" \
    "1 apt package(s)" "2 not checkable"

# --------------------------------------------------------------- continuations
# A package on a continuation line is still installed by the job, so it is still
# the gate's business. Silent under-coverage reads exactly like a clean result.
write_workflow continuation ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake \
            libfcitx5-dev \
            libglib2.0-dev
YAML
expect "packages after a line continuation are checked" 1 continuation \
    "libfcitx5-dev" "ci.yml:2"

# A file that ends mid-continuation swallowed its packages into the join. Say so
# rather than counting them as examined.
write_workflow dangling ci.yml <<'YAML'
        run: sudo apt-get install -y cmake
        # the next line ends the file inside a continuation
        run: sudo apt-get install -y libglib2.0-dev \
YAML
expect "a file ending inside a continuation is announced" 0 dangling \
    "ends inside a line continuation"

# ------------------------------------------------- a gate that cannot run says so
# Each of these yields an empty finding set, which is indistinguishable from a
# clean pass unless it is reported as a failure to run.
expect "a missing workflow directory is a failure, not a pass" 1 does-not-exist \
    "could not run"

mkdir -p "$WORK/empty"
expect "an empty workflow directory is a failure, not a pass" 1 empty \
    "could not run"

write_workflow no-apt ci.yml <<'YAML'
        run: cargo test --workspace
YAML
expect "workflows with no apt packages are a failure, not a pass" 1 no-apt \
    "found nothing to check"

# The two ways the machine, rather than the input, makes every lookup come back
# empty. Both would otherwise render as "every package in the repository is
# broken" — a unanimous verdict manufactured out of no information at all.
expect_path() { # expect_path <label> <PATH> <want-exit> "<paths>" [must-contain ...]
    local label="$1" path_value="$2" want="$3" paths="$4"
    shift 4
    local out got args=() p
    for p in $paths; do args+=("$WORK/$p"); done
    out="$(PATH="$path_value" bash "$CHECK" "${args[@]}" 2>&1)"
    got=$?

    if [ "$got" -ne "$want" ]; then
        fail "$label: expected exit $want, got $got"
        printf '%s\n' "$out" | sed 's/^/    | /' >&2
        return
    fi
    local needle
    for needle in "$@"; do
        if ! printf '%s' "$out" | grep -qF -- "$needle"; then
            fail "$label: output does not mention '$needle'"
            printf '%s\n' "$out" | sed 's/^/    | /' >&2
            return
        fi
    done
    ok "$label"
}

# apt-cache present but answering nothing, as it does before `apt-get update`.
STUB_BIN="$WORK/stub-bin"
mkdir -p "$STUB_BIN"
printf '#!/bin/sh\nexit 0\n' >"$STUB_BIN/apt-cache"
chmod +x "$STUB_BIN/apt-cache"
expect_path "empty apt lists are a failure, not every package being broken" \
    "$STUB_BIN:$PATH" 1 fixed \
    "control package" "could not run"

# apt-cache absent entirely, as on a non-Debian machine. The stub directory
# carries only bash and what the script itself shells out to.
NO_APT_BIN="$WORK/no-apt-bin"
mkdir -p "$NO_APT_BIN"
for tool in bash find sed grep; do
    ln -sf "$(command -v "$tool")" "$NO_APT_BIN/$tool"
done
expect_path "a machine without apt-cache is a failure, not a pass" \
    "$NO_APT_BIN" 1 fixed \
    "apt-cache not found" "could not run"

# ------------------------------------------------------------- other entrances
# The workflows did not invent these lists; they were copied from
# .github/CI_README.md, which told contributors to install exactly the packages
# that do not exist. A gate on one entrance is not a gate, so a path that is a
# FILE rather than a directory is scanned too.
mkdir -p "$WORK/docs"
cat >"$WORK/docs/CI_README.md" <<'MD'
System dependencies (Ubuntu/Debian):

```bash
sudo apt-get install -y \
  cmake g++ \
  libfcitx5-dev libglib2.0-dev
```
MD
expect "a file path is scanned, not just a directory" 1 docs/CI_README.md \
    "libfcitx5-dev" "CI_README.md:4"

# The real invocation passes both at once, so a defect in either has to surface.
expect "several paths are scanned together" 1 "fixed docs/CI_README.md" \
    "libfcitx5-dev"

expect "a missing file path is a failure, not a pass" 1 docs/nope.md \
    "could not run"

# `apt-get update` names no packages and must not be read as an install line.
write_workflow update-only ci.yml <<'YAML'
        run: |
          sudo apt-get update
          sudo apt-get install -y cmake
YAML
expect "apt-get update is not parsed as an install" 0 update-only \
    "All 1 apt package(s)"

# ------------------------------------------------------------------------ done
printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
[ "$FAILED" -eq 0 ]
