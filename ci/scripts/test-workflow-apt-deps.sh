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

# The same line without the second `sudo`. A shell operator is a command
# position in its own right, and requiring `sudo` to recognise one would let
# this form through unexamined.
write_workflow chained-nosudo ci.yml <<'YAML'
        run: sudo apt-get install -y cmake && apt-get install -y libfcitx5-dev
YAML
expect "an operator is a command position on its own" 1 chained-nosudo \
    "libfcitx5-dev"

# A `#` starts a comment that runs to the end of the line. Reading its words as
# packages fails a perfectly good workflow, and this gate blocks PRs.
write_workflow commented ci.yml <<'YAML'
        run: sudo apt-get install -y cmake # needed for the fcitx5 addon
YAML
expect "an inline shell comment is not a package list" 0 commented \
    "All 1 apt package(s)"

# `apt-get [options] install pkg` is documented apt syntax, so matching the
# literal string "apt-get install" walks straight past an invalid package in the
# one form the gate exists to catch.
write_workflow global-opts ci.yml <<'YAML'
        run: sudo apt-get --no-install-recommends install -y libfcitx5-dev
YAML
expect "options between apt-get and install are recognised" 1 global-opts \
    "libfcitx5-dev"

# Scanning prose (CI_README.md) means the words "apt install" can occur in a
# sentence. Only a command position starts a package list — otherwise the next
# sentence becomes a list of packages that do not exist.
mkdir -p "$WORK/prose"
cat >"$WORK/prose/CI_README.md" <<'MD'
If the apt install step fails, check your mirrors first.

```bash
sudo apt-get install -y cmake
```
MD
expect "prose mentioning apt install is not a package list" 0 prose/CI_README.md \
    "All 1 apt package(s)" "looks like an apt install command but was not parsed"

# A name assembled at runtime cannot be resolved here. It must be announced and
# skipped, not guessed at in either direction.
write_workflow interpolated ci.yml <<'YAML'
        run: sudo apt-get install -y cmake ${{ matrix.extra_package }} $EXTRA
YAML
expect "an interpolated package name is announced, not judged" 0 interpolated \
    "not checked: \${{matrix.extra_package}}" "not checked: \$EXTRA" \
    "1 apt package(s)" "2 not checkable"

# A redirection needs no space in front of its target, so `>/dev/null` arrives as
# a single token that none of the bare operator patterns match. A package name
# never contains `>` or `<`.
write_workflow redirect ci.yml <<'YAML'
        run: sudo apt-get install -y cmake >/dev/null 2>&1
YAML
expect "an attached redirection is not a package" 0 redirect \
    "All 1 apt package(s)"

# The rest of the punctuation class Codex found in the comment and option cases:
# shell quoting and a trailing separator are not part of the package name, and
# reading them as one rejects a command that works.
write_workflow punctuation ci.yml <<'YAML'
        run: sudo apt-get install -y "cmake" 'g++' libglib2.0-dev;
YAML
expect "quotes and a trailing separator are not part of the name" 0 punctuation \
    "All 3 apt package(s)"

# An attached `;` ends the command as surely as a spaced one. Stripping it off
# the package name while staying in the package list reads `echo done` as two
# packages and rejects a correct workflow.
write_workflow semicolon ci.yml <<'YAML'
        run: sudo apt-get install -y cmake; echo done
YAML
expect "an attached semicolon ends the command" 0 semicolon \
    "All 1 apt package(s)"

# A comma is NOT shell punctuation: the shell passes it through and apt rejects
# the name. Verified with `apt-get -s install cmake,` -> exit 100, "Unable to
# locate package cmake,". Stripping it would hide exactly the kind of typo this
# gate exists to catch.
write_workflow comma ci.yml <<'YAML'
        run: sudo apt-get install -y cmake, g++
YAML
expect "a comma-suffixed name is rejected, because apt rejects it" 1 comma \
    "cmake,"

# `sudo [options] command` is documented sudo syntax, so `apt-get` does not
# always follow `sudo` directly.
write_workflow sudo-opts ci.yml <<'YAML'
        run: sudo -E apt-get install -y libfcitx5-dev
YAML
expect "sudo options before apt-get are consumed" 1 sudo-opts \
    "libfcitx5-dev"

# The general safety net for this whole class. `sudo -u root apt-get ...` puts an
# option ARGUMENT before the command, which cannot be recognised without knowing
# which options take arguments. Not parsing it is acceptable; not SAYING SO is
# not, because a silent skip reads exactly like a clean result.
write_workflow unparsed ci.yml <<'YAML'
        run: sudo apt-get install -y cmake
        run: sudo -u root apt-get install -y libfcitx5-dev
YAML
expect "an install form the parser cannot read is announced" 0 unparsed \
    "looks like an apt install command but was not parsed" "1 not checkable"

# Quoting that spans a space makes ONE argument, so `"cmake g++"` asks apt for a
# package of that name and gets nothing: `apt-get -s install "cmake g++"` exits
# 100. Stripping the quotes off each whitespace token independently reports two
# good packages for a command that cannot work.
write_workflow quoted-multiword ci.yml <<'YAML'
        run: sudo apt-get install -y "cmake g++"
YAML
expect "a quoted multiword argument is one package name" 1 quoted-multiword \
    "cmake g++"

# A quote with no closing partner cannot be resolved into a name at all, so it is
# announced rather than guessed at in either direction.
write_workflow quote-unclosed ci.yml <<'YAML'
        run: sudo apt-get install -y g++ "cmake
YAML
expect "an unterminated quote is announced" 0 quote-unclosed \
    "All 1 apt package(s)" "1 not checkable"

# Some apt options take a SEPARATE argument, and that argument is not a package.
# Verified against apt 2.8.3: `apt-get -s install -o Debug::NoLocking=1 cmake`
# exits 0.
write_workflow option-arg ci.yml <<'YAML'
        run: sudo apt-get install -o Debug::NoLocking=1 cmake
YAML
expect "an apt option argument is not a package" 0 option-arg \
    "All 1 apt package(s)"

# ...but only the options that actually take one. `--no-install-recommends` does
# not, so consuming the token after it would swallow a real package — the false
# green this gate exists to prevent.
write_workflow option-noarg ci.yml <<'YAML'
        run: sudo apt-get install --no-install-recommends libfcitx5-dev
YAML
expect "an option that takes no argument does not swallow a package" 1 option-noarg \
    "libfcitx5-dev"

# Bash `&>` and `&>>` redirect both streams and are single operators; spacing
# only the `>` leaves a bare `&` to be looked up as a package.
write_workflow combined-redirect ci.yml <<'YAML'
        run: sudo apt-get install -y cmake &>/dev/null
YAML
expect "a combined output redirection is one operator" 0 combined-redirect \
    "All 1 apt package(s)"

# A MULTIWORD substitution has middle tokens carrying neither `$` nor a
# parenthesis, so nothing marks them as uncheckable and `%s` gets looked up as a
# package. The whole substitution is one expansion and none of it is knowable
# here.
write_workflow substitution-multiword ci.yml <<'YAML'
        run: sudo apt-get install -y cmake $(printf '%s' g++)
YAML
expect "every word of a command substitution is announced" 0 substitution-multiword \
    "All 1 apt package(s)" "3 not checkable"

# `$(cat pkgs.txt)` splits into `$(cat` and `pkgs.txt)`. Only the first carries a
# `$`; judging the second as a package name fails a correct workflow.
write_workflow substitution ci.yml <<'YAML'
        run: sudo apt-get install -y cmake $(cat extra-packages.txt)
YAML
expect "a command substitution is announced, not judged" 0 substitution \
    "not checked" "1 apt package(s)" "2 not checkable"

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

# ------------------------------------------- metacharacters need no whitespace
# The shell ends a word at `>` without a space, so `bad-package>/dev/null` still
# installs `bad-package`. Discarding the whole whitespace token throws the
# package away with the redirection.
write_workflow attached-redirect ci.yml <<'YAML'
        run: sudo apt-get install -y cmake bad-package>/dev/null
YAML
expect "a package attached to a redirection is still checked" 1 attached-redirect \
    "bad-package"

# ...but a leading FILE DESCRIPTOR is not a package: `2>&1` is a redirection
# whose word is the digit 2.
write_workflow fd-redirect ci.yml <<'YAML'
        run: sudo apt-get install -y cmake 2>&1
YAML
expect "a file descriptor before a redirection is not a package" 0 fd-redirect \
    "All 1 apt package(s)"

# Same for the boolean operators: the shell ends the word before `&&` whether or
# not a space is written, so `cmake&&` installs `cmake` and starts a new command.
write_workflow attached-and ci.yml <<'YAML'
        run: sudo apt-get install -y cmake&& echo done
YAML
expect "an attached && ends the word and the command" 0 attached-and \
    "All 1 apt package(s)"

# A redirection does not end the argument list: `cmd a 2>/dev/null b` passes
# BOTH a and b. Resetting the parser at the redirection loses everything after
# it.
write_workflow after-redirect ci.yml <<'YAML'
        run: sudo apt-get install -y cmake 2>/dev/null bad-package
YAML
expect "packages after a redirection are still checked" 1 after-redirect \
    "bad-package"

# `>>` is a redirection too, and the two-character form must not be read as two
# separate `>` (which would consume a package as the second one's target).
write_workflow append-redirect ci.yml <<'YAML'
        run: sudo apt-get install -y cmake >>build.log g++
YAML
expect "an append redirection consumes only its target" 0 append-redirect \
    "All 2 apt package(s)"

# ...and a separator with no space after it still starts a real command, whose
# packages are as much the gate's business as the first command's.
write_workflow attached-next ci.yml <<'YAML'
        run: sudo apt-get install -y cmake;apt-get install -y bad-package
YAML
expect "a command attached to a separator is parsed" 1 attached-next \
    "bad-package"

# ------------------------------------------------------------------- heredocs
# A heredoc body is DATA, not commands: bash writes those lines to a file and
# runs none of them. Reading them as commands rejects a workflow that generates
# an install script, which is a false red on a gate that blocks PRs.
write_workflow heredoc ci.yml <<'YAML'
        run: |
          cat > install-example.sh <<'EOF'
          sudo apt-get install -y codex-no-such-package
          EOF
          sudo apt-get install -y cmake
YAML
expect "a heredoc body is not a command" 0 heredoc \
    "All 1 apt package(s)" "inside a heredoc"

# ------------------------------------------------------ YAML folded run blocks
# `run: >` folds the following more-indented lines into ONE command, so the
# package can sit on a line that names no command at all.
write_workflow folded ci.yml <<'YAML'
      - name: ok
        run: sudo apt-get install -y cmake
      - name: folded
        run: >
          sudo apt-get install -y
          libfcitx5-dev
YAML
expect "a folded run block is one command" 1 folded \
    "libfcitx5-dev" "ci.yml:4"

# YAML does not fold a MORE-indented line into the line above it: the newline is
# kept, so it is its own command. Joining it with a space hides the command it
# holds.
write_workflow folded-more-indented ci.yml <<'YAML'
      - name: ok
        run: sudo apt-get install -y cmake
      - name: folded
        run: >
          echo first
            sudo apt-get install -y libfcitx5-dev
YAML
expect "a more-indented folded line is its own command" 1 folded-more-indented \
    "libfcitx5-dev" "ci.yml:6"

# The fold ends where the indentation drops, or the next step would be glued to
# the previous command and its words read as packages.
write_workflow folded-end ci.yml <<'YAML'
      - name: folded
        run: >
          sudo apt-get install -y
          cmake
      - name: after
        run: cargo test --workspace
YAML
expect "a folded block ends when the indentation drops" 0 folded-end \
    "All 1 apt package(s)"

# YAML allows a comment after the scalar indicator, and it does not stop the
# block from being a folded scalar.
write_workflow folded-comment ci.yml <<'YAML'
      - name: folded
        run: > # install the addon dependencies
          sudo apt-get install -y
          libfcitx5-dev
YAML
expect "a comment on a folded header still folds" 1 folded-comment \
    "libfcitx5-dev"

# An explicit indentation indicator sets the folding baseline, so lines deeper
# than IT are more-indented and keep their newlines — even though they all share
# one indentation with each other. Confirmed against PyYAML, which preserves the
# newline between them.
write_workflow folded-indicator-baseline ci.yml <<'YAML'
      - name: ok
        run: sudo apt-get install -y cmake
      - run: >2
            echo first
            sudo apt-get install -y libfcitx5-dev
YAML
expect "an explicit indentation indicator sets the fold baseline" 1 folded-indicator-baseline \
    "libfcitx5-dev"

# The indicator counts from the PARENT NODE, which starts where the key does —
# past the dash, not at it. With the key at column 8 and `>2`, content at column
# 10 is exactly at the baseline, so both lines fold into one command and the
# `apt-get` in the second is an argument to `echo`, not an install. PyYAML agrees:
# 'echo first sudo apt-get install -y libfcitx5-dev\n'. Measuring the dash column
# instead would make these two more-indented lines and invent an install.
write_workflow folded-indicator-exact ci.yml <<'YAML'
      - name: ok
        run: sudo apt-get install -y cmake
      - run: >2
          echo first
          sudo apt-get install -y libfcitx5-dev
YAML
expect "content at the indicator baseline folds into one command" 0 folded-indicator-exact \
    "All 1 apt package(s)" "looks like an apt install command but was not parsed"

# YAML puts the first key of a sequence item on the dash line, so `- run: >` is
# the same fold with the key one step to the right.
write_workflow folded-dash ci.yml <<'YAML'
      - run: >
          sudo apt-get install -y
          libfcitx5-dev
YAML
expect "a fold opened on the sequence-dash line is recognised" 1 folded-dash \
    "libfcitx5-dev"

# YAML allows an explicit indentation indicator on a block scalar, alone or
# combined with a chomping indicator: `>2`, `>2-`, `>-2`.
write_workflow folded-indent ci.yml <<'YAML'
      - name: folded
        run: >2
          sudo apt-get install -y
          libfcitx5-dev
YAML
expect "a folded header with an indentation indicator still folds" 1 folded-indent \
    "libfcitx5-dev"

write_workflow folded-indent-chomp ci.yml <<'YAML'
      - name: folded
        run: >2-
          sudo apt-get install -y
          libfcitx5-dev
YAML
expect "an indentation and chomping indicator together still fold" 1 folded-indent-chomp \
    "libfcitx5-dev"

# `run: |` is LITERAL: newlines are kept, so each line is its own command and
# must not be folded together.
write_workflow literal-block ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          echo done
YAML
expect "a literal run block keeps its lines separate" 0 literal-block \
    "All 1 apt package(s)"

# `run: echo a > b` is a command containing a redirection, not a folded scalar.
write_workflow not-folded ci.yml <<'YAML'
        run: echo hello > /tmp/x
        run: sudo apt-get install -y cmake
YAML
expect "a redirection in a run command is not a folded scalar" 0 not-folded \
    "All 1 apt package(s)"

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
expect_path "an apt-cache that answers nothing is a failure, not a pass" \
    "$STUB_BIN:$PATH" 1 fixed \
    "no repository indexes" "could not run"

# apt-cache present and answering, but from /var/lib/dpkg/status alone because no
# lists have been fetched. This is the case a control PACKAGE cannot see: an
# installed package still reports a candidate, so the guard concludes the lists
# are fine and every uninstalled dependency is then reported as nonexistent.
# Real apt against an empty lists directory, not a mock, so the reproduction is
# the actual condition.
EMPTY_LISTS="$WORK/empty-lists"
mkdir -p "$EMPTY_LISTS"
UNFETCHED_BIN="$WORK/unfetched-bin"
mkdir -p "$UNFETCHED_BIN"
cat >"$UNFETCHED_BIN/apt-cache" <<EOF
#!/bin/sh
exec "$(command -v apt-cache)" -o Dir::State::Lists="$EMPTY_LISTS" "\$@"
EOF
chmod +x "$UNFETCHED_BIN/apt-cache"
expect_path "unfetched apt lists are a failure, though installed packages resolve" \
    "$UNFETCHED_BIN:$PATH" 1 fixed \
    "could not run"

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

# ------------------------------------------------------------- one call per name
# 149 occurrences of 14 distinct names meant 149 `apt-cache` processes. Nothing
# about a package's candidate changes within a run, so the answers are cached —
# pinned by counting the processes rather than by timing them, which would be a
# flaky assertion about whatever machine is running the suite.
write_workflow repeated ci.yml <<'YAML'
        run: sudo apt-get install -y cmake cmake cmake g++ cmake g++
YAML
CALL_LOG="$WORK/apt-calls"
COUNTING_BIN="$WORK/counting-bin"
mkdir -p "$COUNTING_BIN"
cat >"$COUNTING_BIN/apt-cache" <<EOF
#!/bin/sh
echo "\$*" >>"$CALL_LOG"
exec "$(command -v apt-cache)" "\$@"
EOF
chmod +x "$COUNTING_BIN/apt-cache"
: >"$CALL_LOG"
if PATH="$COUNTING_BIN:$PATH" bash "$CHECK" "$WORK/repeated" >/dev/null 2>&1; then
    lookups="$(grep -c '^policy .' "$CALL_LOG" || true)"
    if [ "$lookups" -eq 2 ]; then
        ok "each distinct package is looked up exactly once"
    else
        fail "each distinct package is looked up exactly once: 6 occurrences of 2 names caused $lookups lookups"
    fi
else
    fail "each distinct package is looked up exactly once: the check did not pass on a valid fixture"
fi

# ------------------------------------------------------------------------ done
printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
[ "$FAILED" -eq 0 ]
