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
# A BLOCK scalar: in a plain YAML scalar a `#` after whitespace is a YAML
# comment, which YAML strips before the scanner ever sees it — the case would
# pass without the shell's comment rule being exercised at all. This test was
# written against a line-reading implementation, where that distinction did not
# exist, and quietly stopped testing anything when the scanner moved to PyYAML.
write_workflow commented ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake # needed for the fcitx5 addon
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
    "not checked: \${{ matrix.extra_package }}" "not checked: \$EXTRA" \
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
        run: |
          sudo apt-get install -y g++
          sudo apt-get install -y "cmake
YAML
expect "an unterminated quote is announced" 0 quote-unclosed \
    "All 1 apt package(s)" "could not be tokenised" "1 not checkable"

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
    "All 1 apt package(s)" "1 not checkable" \
    "not checked: \$(printf %s g++)"

# `$(cat pkgs.txt)` splits into `$(cat` and `pkgs.txt)`. Only the first carries a
# `$`; judging the second as a package name fails a correct workflow.
write_workflow substitution ci.yml <<'YAML'
        run: sudo apt-get install -y cmake $(cat extra-packages.txt)
YAML
expect "a command substitution is announced, not judged" 0 substitution \
    "not checked" "1 apt package(s)" "1 not checkable"

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

# A trailing backslash with nothing after it continues into nothing, so the
# command is complete and its packages are still checked. The bash reader used to
# swallow this line and announce it unexamined; assembling the logical line makes
# the announcement unnecessary.
write_workflow dangling ci.yml <<'YAML'
        run: sudo apt-get install -y cmake
        run: sudo apt-get install -y libglib2.0-dev \
YAML
expect "a trailing continuation still has its packages checked" 0 dangling \
    "All 2 apt package(s)"

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

# A heredoc delimiter is a shell WORD, not an identifier: `<<'END-MARKER'` ends
# at a line saying END-MARKER. Capturing only `END` means the terminator never
# matches and every command after it is swallowed as heredoc data.
write_workflow heredoc-punct ci.yml <<'YAML'
        run: |
          cat > x.sh <<'END-MARKER'
          sudo apt-get install -y codex-no-such-package
          END-MARKER
          sudo apt-get install -y cmake
YAML
expect "a punctuated heredoc delimiter still closes the body" 0 heredoc-punct \
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
    "libfcitx5-dev" "ci.yml:4"

# A blank line inside a folded block is a paragraph break, NOT the end of the
# block: YAML keeps folding the lines after it. Confirmed against PyYAML:
# 'echo first\nsudo apt-get install -y codex-no-such-package\n'.
write_workflow folded-blank ci.yml <<'YAML'
      - name: ok
        run: sudo apt-get install -y cmake
      - run: >
          echo first

          sudo apt-get install -y
          codex-no-such-package
YAML
expect "a blank line does not close a folded block" 1 folded-blank \
    "codex-no-such-package"

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

# ------------------------------------------------ what the libraries buy us
# A metacharacter INSIDE quotes is a literal character, not an operator, so
# `"codex;no-such-package"` is one package name and apt rejects it (exit 100).
# The hand-written tokeniser split it and passed the workflow; shlex does not.
write_workflow quoted-metachar ci.yml <<'YAML'
        run: sudo apt-get install -y cmake "codex;no-such-package"
YAML
expect "a metacharacter inside quotes is part of the name" 1 quoted-metachar \
    "codex;no-such-package"

# Shell does not only live under `run:`. desktop-app-release.yml keeps an install
# in a matrix entry and executes it later through `run: ${{ matrix.extra_deps }}`,
# so a scan that trusts the key name misses it — which is exactly what the first
# version of the library-based scanner did, caught by comparing package counts
# against the implementation it replaced.
write_workflow matrix-value ci.yml <<'YAML'
      - os: linux
        extra_deps: |
          sudo apt-get update
          sudo apt-get install -y libfcitx5-dev
        run: ${{ matrix.extra_deps }}
YAML
expect "a package named in a matrix value is checked" 1 matrix-value \
    "libfcitx5-dev"

# A file that does not parse is not a file with no packages.
mkdir -p "$WORK/broken-yaml"
printf 'steps:\n  - run: "unterminated\n   bad: [\n' >"$WORK/broken-yaml/ci.yml"
expect "a workflow that is not valid YAML is a failure, not a pass" 1 broken-yaml \
    "could not run"

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

# A missing path alongside VALID ones. Without this the missing-path guard is
# covered only by a case where nothing else is scanned either, and skipping the
# bad path would still fail on "found nothing to check" — a different guard
# catching it, which is not the same as this one working.
expect "a missing path among valid ones is still a failure" 1 "fixed does-not-exist" \
    "does not exist" "could not run"

# PyYAML missing is its own condition, and without the guard the scanner merely
# crashes — still non-zero, but reported as an unexplained failure. A shim that
# shadows the real module reproduces it exactly.
NO_YAML="$WORK/no-yaml"
mkdir -p "$NO_YAML"
printf 'raise ImportError("not available in this test")\n' >"$NO_YAML/yaml.py"
out="$(PYTHONPATH="$NO_YAML" "$CHECK" "$WORK/fixed" 2>&1)"
got=$?
if [ "$got" -eq 1 ] && printf '%s' "$out" | grep -qF "PyYAML not available"; then
    ok "a missing PyYAML is named, not left as an unexplained crash"
else
    fail "a missing PyYAML is named, not left as an unexplained crash: exit $got"
    printf '%s\n' "$out" | sed 's/^/    | /' >&2
fi

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

# ------------------------------------------- heredocs the shell does not open
# `echo example # <<EOF` opens no heredoc, because everything after `#` is a
# comment. A scanner that searches the raw line for `<<` opens one anyway and
# then swallows every command after it as data — silently under-checking a file
# that looks clean. shlex drops the comment for us; this pins that it stays
# dropped.
write_workflow heredoc-commented ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          echo example # <<EOF
          sudo apt-get install -y codex-no-such-package
YAML
# The valid package is load-bearing: with none, the gate exits 1 through its
# "nothing to check" guard and the case would pass without reading the comment
# correctly at all.
expect "a commented-out heredoc marker opens no heredoc" 1 heredoc-commented \
    "codex-no-such-package" "installs a package apt cannot resolve"

# `<<-EOF` is a heredoc whose terminator may be indented with tabs, and bash
# resumes executing at the line after it. The dash belongs to the OPERATOR, not
# to the delimiter word: read as `-EOF` the terminator never matches, the
# heredoc never closes, and every install after it is announced as data instead
# of checked.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake' \
    '          cat <<-EOF > /tmp/generated' \
    '          body text' \
    '          	EOF' \
    '          sudo apt-get install -y codex-no-such-package' \
    | write_workflow heredoc-dash ci.yml
expect "a <<- heredoc closes on its tab-indented terminator" 1 heredoc-dash \
    "codex-no-such-package" "installs a package apt cannot resolve"

# bash allows whitespace between the redirection operator and its word, so
# `<<- EOF` is the same heredoc. The dash then arrives on its own: taken as the
# delimiter it yields an empty one, which closes the body on the first blank
# line and reads the rest of the data as commands.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake' \
    '          cat <<- EOF > /tmp/generated' \
    '' \
    '          sudo apt-get install -y codex-no-such-package' \
    '          	EOF' \
    | write_workflow heredoc-dash-spaced ci.yml
expect "a <<- heredoc with a spaced delimiter still has a delimiter" 0 heredoc-dash-spaced \
    "inside a heredoc" "All 1 apt package(s)"

# The shell joins no line continuations inside a heredoc body: the body is
# data, and `line one \` does not swallow the `EOF` beneath it. Joining them
# first leaves the heredoc open forever, and every command after it is
# announced as data instead of checked.
write_workflow heredoc-body-continuation ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          cat > x.sh <<'EOF'
          line one \
          EOF
          sudo apt-get install -y codex-no-such-package
YAML
expect "a continuation inside a heredoc body does not swallow the terminator" 1 heredoc-body-continuation \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...and the terminator is the line, not the word on it. A `<<` heredoc ends on
# a line that is EXACTLY the delimiter; an indented one is body text, which is
# why the whole line is compared and not a trimmed copy of it.
write_workflow heredoc-indented-terminator ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          cat > x.sh <<'EOF'
            EOF
          sudo apt-get install -y codex-no-such-package
          EOF
YAML
expect "an indented terminator does not close a plain heredoc" 0 heredoc-indented-terminator \
    "inside a heredoc" "All 1 apt package(s)"

# One command may open more than one heredoc, and the shell reads their bodies
# in order: `cat <<A <<B` reads all of A, then all of B. Keeping only the first
# resumes reading commands while the shell is still reading data.
write_workflow heredoc-two ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          cat <<A <<B > /tmp/generated
          body of a
          A
          sudo apt-get install -y codex-no-such-package
          B
YAML
expect "a second heredoc on the same command is still a heredoc" 0 heredoc-two \
    "inside a heredoc" "All 1 apt package(s)"

# ------------------------------------------------------- literal dollar signs
# `'$MISSING'` is not a variable reference. The shell performs no expansion
# inside single quotes, so apt receives the eight characters `$MISSING`, fails
# to find them and exits 100. Announcing it as an unresolvable variable lets the
# broken install through the gate.
write_workflow dollar-single ci.yml <<'YAML'
        run: sudo apt-get install -y cmake '$MISSING'
YAML
expect "a single-quoted dollar is a literal package name" 1 dollar-single \
    '$MISSING'

# Same for a backslash-escaped `$`, for the same reason.
write_workflow dollar-escaped ci.yml <<'YAML'
        run: sudo apt-get install -y cmake \$ESCAPED
YAML
expect "a backslash-escaped dollar is a literal package name" 1 dollar-escaped \
    '$ESCAPED'

# The other direction, and the reason this cannot be settled by looking for a
# `$`: double quotes DO expand. Judging `"$DQ"` as a package name would be a
# false red on a workflow that is perfectly correct.
write_workflow dollar-double ci.yml <<'YAML'
        run: sudo apt-get install -y cmake "$DQ"
YAML
expect "a double-quoted dollar is still a runtime variable" 0 dollar-double \
    "not checked" "All 1 apt package(s)"

# Unquoted, likewise.
write_workflow dollar-bare ci.yml <<'YAML'
        run: sudo apt-get install -y cmake $BARE
YAML
expect "an unquoted dollar is still a runtime variable" 0 dollar-bare \
    "not checked" "All 1 apt package(s)"

# The quoting is read out of the lexer's own state rather than reconstructed
# here, so this pins the four states it reports. If a future Python changes
# them, this fails loudly instead of the gate quietly reclassifying every
# literal `$` as a variable again — the direction that lets a broken install
# through.
if SCRIPT_DIR="$SCRIPT_DIR" python3 - <<'PYPIN'
import os
import sys
sys.path.insert(0, os.environ["SCRIPT_DIR"])
from workflow_apt_deps import lex_words

for text, want in [
    ("a '$X'", [False, True]),
    ("a \\$X", [False, True]),
    ("a $X", [False, False]),
    ('a "$X"', [False, False]),
]:
    got = [word.literal_dollar for word in lex_words(text)]
    if got != want:
        print(f"{text!r}: expected {want}, got {got}")
        raise SystemExit(1)
PYPIN
then
    ok "the lexer reports which dollar signs the shell would not expand"
else
    fail "the lexer reports which dollar signs the shell would not expand"
fi

# ------------------------------------------------ the documented prerequisites
# CI_README.md tells a contributor what to install before running test-all.sh,
# and test-all.sh now runs this gate, which needs PyYAML. The workflows were
# updated; a fresh machine following the README alone was not, and fails on its
# first run with an error about a package nobody told it to install.
if grep -q 'python3-yaml' "$SCRIPT_DIR/../../.github/CI_README.md"; then
    ok "the documented local-development dependencies include PyYAML"
else
    fail "the documented local-development dependencies include PyYAML"
fi


# `<< -EOF` is NOT the `<<-` operator. The dash is separated from `<<` by
# whitespace, so it belongs to the delimiter WORD: bash ends this body on a line
# saying `-EOF` and strips no tabs. Stripping the dash off every delimiter that
# starts with one — the shape of the `<<-EOF` fix — misses that terminator and
# swallows the rest of the block as data. shlex tokenises both forms
# identically, so the adjacency has to come from the lexer's reading position.
write_workflow heredoc-spaced-dash-delimiter ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          cat << -EOF > /tmp/generated
          body text
          -EOF
          sudo apt-get install -y codex-no-such-package
YAML
expect "a space before a dashed delimiter keeps the dash" 1 heredoc-spaced-dash-delimiter \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ---------------------------------------------- substitutions inside a word
# A command substitution need not be the whole argument: `c$(printf make)` is
# one word that expands to `cmake`, and apt installs it. shlex hands back `c$`,
# `(`, `printf`, `make`, `)` — reporting those four as package names rejects a
# workflow that works, which is the failure that gets a gate switched off.
write_workflow substitution-embedded ci.yml <<'YAML'
        run: sudo apt-get install -y cmake c$(printf make)
YAML
expect "a substitution inside a word is one unresolvable argument" 0 substitution-embedded \
    "All 1 apt package(s)" "not checked"

# shlex groups a RUN of punctuation into one token, so the `)` closing a
# substitution arrives welded to whatever follows it — `);`, `)>`, `)&&`. Read
# as one word it closes nothing, the walk runs to the end of the line, and the
# command after the separator is swallowed without a word being said about it.
# That is silent under-coverage, which reads exactly like a clean result.
write_workflow substitution-then-command ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y $(printf make);sudo apt-get install -y libfcitx5-dev
YAML
# The valid package on the first line is load-bearing: without it the swallowed
# case exits 1 through the "nothing to check" guard, with the swallowed name
# quoted in the notice, and this would pass while the command was being lost.
expect "a command after a substitution is not swallowed" 1 substitution-then-command \
    "libfcitx5-dev" "installs a package apt cannot resolve"

# ...and the other direction: absorbing the separator INTO the substitution
# leaves the words after it being read as a package list, so a workflow that
# installs one package and then echoes is rejected for installing `echo`.
write_workflow substitution-then-separator ci.yml <<'YAML'
        run: sudo apt-get install -y cmake $(printf make);echo done
YAML
expect "a separator after a substitution ends the package list" 0 substitution-then-separator \
    "All 1 apt package(s)"

# `;;` is a welded run too, and it is neither a package nor an operator this
# scanner knows. Split into two separators it does what it does — end the
# command — where kept whole it is looked up as a package name and rejects a
# workflow containing a `case` statement.
write_workflow double-semicolon ci.yml <<'YAML'
        run: sudo apt-get install -y cmake;;echo x
YAML
expect "a doubled separator is separators, not a package" 0 double-semicolon \
    "All 1 apt package(s)"

# Same weld, redirection instead of a separator: the target of the redirect is
# not part of the package argument either.
write_workflow substitution-then-redirect ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y c$(printf make)>/dev/null
YAML
expect "a redirection after a substitution is not part of it" 0 substitution-then-redirect \
    "All 1 apt package(s)" "c\$(printf make)"

# ------------------------------------------------ continuations join nothing
# A backslash-newline is REMOVED, not replaced by a space: `cma\` followed by
# `ke` is the single word `cmake`. Joining with a space asks apt for `cma` and
# `ke`, neither of which exists, and blocks a workflow that installs correctly.
# The conventional layout is unaffected — its separator is the whitespace
# already sitting before the backslash, or the next line's indentation.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cma\' \
    '          ke' \
    | write_workflow continuation-midword ci.yml
expect "a continuation inside a word joins it up" 0 continuation-midword \
    "All 1 apt package(s)"


# ------------------------------------------- a heredoc feeding apt ITSELF
# `apt-get install -y cmake <<EOF` redirects a heredoc INTO apt. `<<` is an
# operator there, not a package, and neither is the delimiter after it —
# reporting them rejects a workflow that installs correctly.
write_workflow heredoc-into-apt ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake <<EOF
          EOF
YAML
expect "a heredoc redirected into apt is not a package" 0 heredoc-into-apt \
    "All 1 apt package(s)"

# ...and the operator's dash is part of the operator here too.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake <<- EOF' \
    '          	EOF' \
    | write_workflow heredoc-into-apt-dash ci.yml
expect "a dashed heredoc into apt is not a package either" 0 heredoc-into-apt-dash \
    "All 1 apt package(s)"

# ------------------------------------------- comments end at the line, always
# A backslash at the end of a COMMENT continues nothing: bash has already
# discarded the rest of the line, and the command beneath it runs on its own.
# Joining first makes the install part of the comment, and it disappears
# without so much as a notice — silent under-coverage, which reads exactly like
# a clean result.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake' \
    '          # comment \' \
    '          sudo apt-get install -y codex-no-such-package' \
    | write_workflow comment-continuation ci.yml
expect "a backslash ending a comment continues nothing" 1 comment-continuation \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...while a backslash ending a line whose `#` is QUOTED is a real
# continuation, because that `#` never started a comment.
printf '%s\n' \
    '        run: |' \
    "          sudo apt-get install -y '#' \\" \
    '          codex-no-such-package' \
    | write_workflow quoted-hash-continuation ci.yml
expect "a quoted hash does not make the line a comment" 1 quoted-hash-continuation \
    "codex-no-such-package"

# ------------------------------------------------- backtick substitutions
# `` c`printf make` `` is one word that expands to `cmake`. shlex leaves the
# backticks embedded in two whitespace-separated tokens, and reporting those as
# package names blocks a workflow that works.
# The words INSIDE it matter: `printf %s make` contains whitespace, so the
# substitution spans three tokens and the middle one carries no backtick at
# all. Announcing token by token would look right for the outer two and report
# `%s` as a missing package.
write_workflow backtick-substitution ci.yml <<'YAML'
        run: sudo apt-get install -y cmake c`printf %s make`
YAML
expect "a backtick substitution is one unresolvable argument" 0 backtick-substitution \
    "All 1 apt package(s)" "not checked"

# ...and it stops at its closing backtick. A walk that runs to the end of the
# line swallows whatever follows the substitution, including a whole second
# install command, and says nothing about it.
write_workflow backtick-then-command ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y `printf make`;sudo apt-get install -y libfcitx5-dev
YAML
expect "a command after a backtick substitution is not swallowed" 1 backtick-then-command \
    "libfcitx5-dev" "installs a package apt cannot resolve"

# The other direction, twice over. Single quotes suppress the substitution, so
# those backticks are characters in a package name and apt rejects it...
write_workflow backtick-single-quoted ci.yml <<'YAML'
        run: sudo apt-get install -y cmake '`codex-no-such-package`'
YAML
expect "single-quoted backticks are part of the name" 1 backtick-single-quoted \
    "installs a package apt cannot resolve"

# ...but DOUBLE quotes do not: the shell still runs the command inside them.
write_workflow backtick-double-quoted ci.yml <<'YAML'
        run: sudo apt-get install -y cmake "`printf make`"
YAML
expect "double-quoted backticks still substitute" 0 backtick-double-quoted \
    "All 1 apt package(s)" "not checked"


# ------------------------------------------- a hash is only sometimes a comment
# Bash starts a comment at `#` only where a WORD can begin. After other
# characters it is an ordinary character: `cmake#typo` is a package name, and
# apt rejects it. Letting the lexer treat every `#` as a comment truncates the
# word to `cmake`, which resolves, and the broken install passes the gate.
write_workflow hash-inside-word ci.yml <<'YAML'
        run: sudo apt-get install -y cmake#typo
YAML
expect "a hash inside a word is part of the name" 1 hash-inside-word \
    "cmake#typo" "installs a package apt cannot resolve"

# ...and where a word DOES begin it is still a comment, which is the half that
# stops the gate inventing packages out of prose.
# A BLOCK scalar, because `#` after a space in a plain YAML scalar is a YAML
# comment: YAML would strip it and the case would pass without the shell rule
# being exercised at all.
write_workflow hash-at-word-start ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake # codex-no-such-package
YAML
expect "a hash starting a word is still a comment" 0 hash-at-word-start \
    "All 1 apt package(s)"

# -------------------------------------------------- quoted operators are words
# `'<<'` is an argument, not a redirection: bash passes it to the command and
# opens no heredoc. shlex removes the quotes, so the token is indistinguishable
# from the operator unless the quoting is tracked — and a heredoc opened here
# never closes, swallowing every command after it as data.
write_workflow quoted-heredoc-operator ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          printf '%s\n' '<<' EOF
          sudo apt-get install -y codex-no-such-package
YAML
expect "a quoted heredoc operator opens no heredoc" 1 quoted-heredoc-operator \
    "codex-no-such-package" "installs a package apt cannot resolve"

# Same for a separator. `';'` is a package name apt will reject, not the end of
# the package list — dropping it loses a broken install without a word.
write_workflow quoted-separator ci.yml <<'YAML'
        run: sudo apt-get install -y cmake ';'
YAML
expect "a quoted separator is a package name" 1 quoted-separator \
    "installs a package apt cannot resolve"

# ------------------------------------------------ apt invoked by absolute path
# `/usr/bin/apt-get install -y ...` is the same command. Matching the name
# exactly missed it entirely — not even the "looks like an apt install command"
# notice fired, because that test was exact too, so the packages went
# unexamined and unmentioned.
write_workflow apt-by-path ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          /usr/bin/apt-get install -y codex-no-such-package
YAML
expect "apt invoked by absolute path is still apt" 1 apt-by-path \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...and nothing else is. A different program whose path merely ends in a
# similar name installs nothing, and reading its arguments as packages would be
# a false red.
write_workflow not-apt-by-path ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          /usr/bin/aptitude install -y codex-no-such-package
YAML
expect "a different program by path is not apt" 0 not-apt-by-path \
    "All 1 apt package(s)"


# ------------------------------------------- comments that cannot be lexed
# A comment may contain an apostrophe, which no lexer can read as shell. The
# first version of this fix handed the whole line to shlex's own comment
# handling to get past it — and that truncates a `#` ANYWHERE in a word, so
# `cmake#typo` became the resolvable `cmake` and the broken install passed
# again, by the back door.
printf '%s\n' \
    '        run: |' \
    "          sudo apt-get install -y cmake#typo # don't use this" \
    | write_workflow comment-apostrophe ci.yml
expect "an unlexable comment does not rescue a broken name" 1 comment-apostrophe \
    "cmake#typo" "installs a package apt cannot resolve"

# ...and a genuinely unbalanced quote in the COMMAND is still announced, which
# is what stops the recovery above from swallowing real breakage.
write_workflow quote-unclosed-still ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y 'codex-no-such-package
YAML
expect "an unterminated quote in the command is still announced" 0 quote-unclosed-still \
    "could not be tokenised" "All 1 apt package(s)"

# --------------------------------------------- backslashes come in pairs
# `foo\\` ends with an ESCAPED backslash, not a continuation: bash keeps the
# newline and runs the next line as its own command. Counting only the last
# character joins them, and the install disappears into the middle of the
# previous line.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake' \
    "          printf '%s' foo\\\\" \
    '          sudo apt-get install -y codex-no-such-package' \
    | write_workflow escaped-backslash ci.yml
expect "an escaped backslash does not continue the line" 1 escaped-backslash \
    "codex-no-such-package" "installs a package apt cannot resolve"


# --------------------------------- the two comment rules, meeting each other
# A comment holding an apostrophe AND ending in a backslash. Each half was
# fixed on its own; the continuation test still answered "no comment" whenever
# the line could not be lexed, so the lines were joined and the install
# vanished into the comment above it — no error, no notice.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake' \
    "          echo ok # don't \\" \
    '          sudo apt-get install -y codex-no-such-package' \
    | write_workflow comment-apostrophe-continuation ci.yml
expect "an unlexable comment still ends its line" 1 comment-apostrophe-continuation \
    "codex-no-such-package" "installs a package apt cannot resolve"

# A `#` may also open a comment immediately after an operator, where a new word
# begins without any whitespace.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake' \
    "          sudo apt-get install -y codex-no-such-package;# don't" \
    | write_workflow hash-after-separator ci.yml
expect "a hash after a separator starts a comment" 1 hash-after-separator \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ----------------------------------- backslashes are literal in single quotes
# Bash removes a backslash-newline pair only where the backslash is an escape.
# Inside single quotes it is an ordinary character, so `'cma\` + `ke'` is ONE
# argument containing a backslash and a newline — a name apt rejects. Counting
# parity alone joins the lines into the perfectly valid `cmake` and the broken
# install passes.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake' \
    "          sudo apt-get install -y 'cma\\" \
    "          ke'" \
    | write_workflow backslash-in-single-quotes ci.yml
expect "a backslash inside single quotes continues nothing" 0 backslash-in-single-quotes \
    "could not be tokenised"

# ...and double quotes are the other half of that rule: inside them a
# backslash-newline IS removed, so `"cma\` + `ke"` is the one word `cmake` and
# the workflow installs correctly. Treating every unclosed quote as a stopper
# rejects it.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y "cma\' \
    '          ke"' \
    | write_workflow backslash-in-double-quotes ci.yml
expect "a backslash inside double quotes still continues" 0 backslash-in-double-quotes \
    "All 1 apt package(s)"

# A comment can hold an unterminated DOUBLE quote as well, and it still ends at
# the newline. Here the continuation test cannot settle it — the line does not
# end inside single quotes — so the comment test has to, using the same words
# the lexer read before it gave up.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake' \
    '          echo # "unterminated \' \
    '          sudo apt-get install -y codex-no-such-package' \
    | write_workflow comment-double-quote-continuation ci.yml
expect "a comment holding a double quote still ends its line" 1 comment-double-quote-continuation \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ------------------------------------- expressions inside a larger word
# `lib${{ matrix.flavor }}` is one package name the workflow builds at run
# time. Recognising the masked expression only when it IS the whole word
# reports the sentinel as a package, and apt cannot resolve something this
# scanner invented — a false red on a matrix-driven workflow that works.
write_workflow expression-embedded ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y lib${{ matrix.flavor }}
          sudo apt-get install -y ${{ matrix.prefix }}-dev
YAML
expect "an expression inside a word is one dynamic argument" 0 expression-embedded \
    "All 1 apt package(s)" "lib\${{ matrix.flavor }}" "\${{ matrix.prefix }}-dev"


# --------------------------------------- a dollar sign is not always a dollar
# `$` only begins an expansion when something can follow it: a name, `{`, `(`,
# or one of the special parameters. At the end of a word it is an ordinary
# character, and apt is handed a name it rejects. Excusing every word that
# merely CONTAINS a `$` lets that through as an unresolvable variable.
write_workflow dollar-trailing ci.yml <<'YAML'
        run: sudo apt-get install -y cmake codex-no-such-package$
YAML
expect "a trailing dollar is part of the name" 1 dollar-trailing \
    "installs a package apt cannot resolve"

# ...while the forms that DO expand are still excused, which is the half that
# keeps the gate off working matrix workflows.
write_workflow dollar-forms ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake $NAME
          sudo apt-get install -y cmake ${BRACED}
          sudo apt-get install -y cmake $1
YAML
expect "the expanding dollar forms are still excused" 0 dollar-forms \
    "All 3 apt package(s)" "not checked: \$NAME" "not checked: \${BRACED}" "not checked: \$1"

# ------------------------------------------------------- brace expansion
# `lib{asound2,pulse}-dev` is two package names, both of which exist. Resolving
# the unexpanded word asks apt for something no workflow ever installs, and
# blocks a PR over a line that works.
write_workflow brace-expansion ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y lib{asound2,pulse}-dev
YAML
expect "a brace expansion is not a package name" 0 brace-expansion \
    "All 1 apt package(s)" "not checked"

# ...but bash expands braces only where there is something to expand: `{only}`
# has no comma and no range, so the word is passed through unchanged and apt
# rejects it. Excusing every brace would hide that.
write_workflow brace-single ci.yml <<'YAML'
        run: sudo apt-get install -y lib{only}-dev
YAML
expect "a brace with nothing to expand is a package name" 1 brace-single \
    "installs a package apt cannot resolve"

# ...and quoting suppresses it entirely: `'{a,b}'` is one literal argument, so
# apt is handed a name with braces in it and rejects it.
write_workflow brace-quoted ci.yml <<'YAML'
        run: sudo apt-get install -y cmake '{asound2,pulse}'
YAML
expect "quoted braces are part of the name" 1 brace-quoted \
    "installs a package apt cannot resolve"


# ------------------------------------ the rest of bash's word expansions
# Found by working through the list of expansions bash performs on a word
# rather than waiting for each to be reported. None of these three is a package
# NAME, and resolving them verbatim is a false red on a workflow that works.

# A path is not a name: apt reads an argument holding a `/` or ending in `.deb`
# as a local package FILE, and Debian names cannot contain either.
write_workflow local-deb ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y ./build/idiolect.deb
          sudo apt-get install -y ~/downloads/idiolect.deb
YAML
expect "a local package file is not a name to resolve" 0 local-deb \
    "All 1 apt package(s)" "a local package file"

# ...but `pkg/stable` is the target-release form, not a path, and is still
# looked up — the LIMITATION already recorded for version suffixes.
write_workflow target-release ci.yml <<'YAML'
        run: sudo apt-get install -y codex-no-such-package/stable
YAML
expect "a target-release suffix is still looked up" 1 target-release \
    "installs a package apt cannot resolve"

# A glob is a pattern, which apt itself also accepts — either way this script
# cannot say which names it stands for.
write_workflow glob-pattern ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y 'libfcitx5*'
YAML
expect "a glob pattern is announced, not resolved" 0 glob-pattern \
    "All 1 apt package(s)" "not checked"

# Process substitution is an operator and a command, not a package list:
# `<(echo x)` becomes a /dev/fd path. Reading its words as packages reported
# `echo`, `x` and `)` as missing.
write_workflow process-substitution ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y <(echo codex-no-such-package)
YAML
# The BARE form on purpose: wrapped in `$( )` the substitution rule already
# swallows it, and the case would pass without process substitution being
# recognised at all.
expect "a process substitution is not a package list" 0 process-substitution \
    "All 1 apt package(s)" "not checked"


# ------------------------------------ a number is a descriptor only when attached
# `2>out` redirects; `2 >out` passes `2` as an argument and apt is asked for a
# package called `2`. The descriptor rule has to require adjacency, exactly as
# the `<<-` one does.
write_workflow fd-spaced ci.yml <<'YAML'
        run: sudo apt-get install -y cmake 2 >out
YAML
expect "a spaced number is an argument, not a descriptor" 1 fd-spaced \
    "installs a package apt cannot resolve"

# ------------------------------------------------- bash's own quoting forms
# `$'...'` is ANSI-C quoting, not an expansion: bash passes the contents
# WITHOUT the dollar. shlex removes the quotes and leaves the `$` welded to the
# text, which then reads exactly like a parameter expansion.
write_workflow dollar-quote ci.yml <<'YAML'
        run: sudo apt-get install -y cmake $'codex-no-such-package'
YAML
expect "ANSI-C quoting is not an expansion" 1 dollar-quote \
    "installs a package apt cannot resolve"

# ...and the dollar has to actually go, not merely stop being excused: the
# contents may be a perfectly good package name, and reporting `$cmake` would
# be a false red on a working workflow.
write_workflow dollar-quote-valid ci.yml <<'YAML'
        run: sudo apt-get install -y $'cmake'
YAML
expect "ANSI-C quoting keeps the name inside it" 0 dollar-quote-valid \
    "All 1 apt package(s)"

# ...and an ESCAPED dollar before a quote is not that form at all: bash passes
# a literal `$` followed by the quoted text, so apt is handed `$cmake` and
# rejects it. Stripping the dollar here would turn a broken install into a
# valid one and let it through.
printf '%s\n' \
    '        run: |' \
    "          sudo apt-get install -y \\\$'cmake'" \
    | write_workflow dollar-quote-escaped ci.yml
expect "an escaped dollar before a quote keeps its dollar" 1 dollar-quote-escaped \
    "installs a package apt cannot resolve"

# ------------------------------------------- quoting one alternative of a brace
# Bash expands `lib{asound2,"pulse"}-dev` — quoting an alternative does not
# suppress the expansion, only quoting the braces or the comma does. A
# word-level "was anything quoted" test says the wrong thing about both.
write_workflow brace-partly-quoted ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y lib{asound2,"pulse"}-dev
YAML
expect "quoting an alternative does not stop the expansion" 0 brace-partly-quoted \
    "All 1 apt package(s)" "not checked"

# ...while quoting the COMMA does stop it, and the literal word is a name apt
# rejects.
write_workflow brace-quoted-comma ci.yml <<'YAML'
        run: sudo apt-get install -y lib{asound2","pulse}-dev
YAML
expect "quoting the comma stops the expansion" 1 brace-quoted-comma \
    "installs a package apt cannot resolve"

# --------------------------------- a process substitution ends at its bracket
# The same walk that swallowed a command after `$( )` and after backticks.
write_workflow process-sub-then-command ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y <(echo x);sudo apt-get install -y libfcitx5-dev
YAML
expect "a command after a process substitution is not swallowed" 1 process-sub-then-command \
    "libfcitx5-dev" "installs a package apt cannot resolve"


# ------------------------------------- quoted brackets inside a substitution
# A `(` inside quotes is an ordinary character, not nesting. Counting it makes
# the walk run past the substitution's real closing bracket and swallow the
# operands after it — the swallowing shape again, this time reachable from
# inside the group rather than after it.
write_workflow substitution-quoted-paren ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake $(printf cmake; : '(') codex-no-such-package
YAML
expect "a quoted bracket does not deepen a substitution" 1 substitution-quoted-paren \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ----------------------------------------------- a tilde is not always a path
# Only `~/` is certainly a home directory. `~name` expands only if that user
# exists, and bash leaves it alone otherwise — quoted, it is never expanded at
# all. Either way apt is handed the literal text and rejects it, so calling
# every leading tilde a local file hides a broken install.
write_workflow tilde-quoted ci.yml <<'YAML'
        run: sudo apt-get install -y cmake '~codex-no-such-package'
YAML
expect "a quoted tilde is part of the name" 1 tilde-quoted \
    "installs a package apt cannot resolve"

write_workflow tilde-bare ci.yml <<'YAML'
        run: sudo apt-get install -y cmake ~codex-no-such-package
YAML
expect "an unquoted tilde without a slash is part of the name" 1 tilde-bare \
    "installs a package apt cannot resolve"

# ...while `~/` really is a path, and resolving it as a package name would be a
# false red on a workflow installing a local build.
write_workflow tilde-home ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y ~/build/idiolect.deb
YAML
expect "a tilde-slash path is still a local file" 0 tilde-home \
    "All 1 apt package(s)" "a local package file"

# A substitution may contain another. Without counting the nesting the walk
# ends on the INNER bracket and the rest of the group is read as packages.
write_workflow substitution-nested ci.yml <<'YAML'
        run: sudo apt-get install -y cmake $(printf %s $(printf make) suffix)
YAML
expect "a nested substitution ends on its own bracket" 0 substitution-nested \
    "All 1 apt package(s)" "not checked"

# The tilde rules, against what apt itself does — checked with `apt-get -s`:
#   /tmp/nosuchdir/pkg  ->  E: Unsupported FILE given on commandline
#   '~/nosuch'          ->  E: Unable to locate PACKAGE ~
# so a quoted `~/` is a name apt rejects, because the shell never expanded it...
write_workflow tilde-quoted-slash ci.yml <<'YAML'
        run: sudo apt-get install -y cmake '~/build/idiolect'
YAML
expect "a quoted tilde-slash is a name, not a path" 1 tilde-quoted-slash \
    "installs a package apt cannot resolve"

# ...while an unquoted one is a path even without a `.deb` suffix.
write_workflow tilde-slash-no-suffix ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y ~/build/idiolect
YAML
expect "an unquoted tilde-slash is a path without a suffix" 0 tilde-slash-no-suffix \
    "All 1 apt package(s)" "a local package file"


# ----------------------------------- the prefilter must read TOKENS, not text
# `a\pt-get` is `apt-get`: a backslash before an ordinary character is just an
# escape. The raw line holds no contiguous "apt", so a substring prefilter
# skipped the command entirely — no packages checked and no notice either,
# which is the one combination this scanner must never produce.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake' \
    '          sudo a\pt-get install -y codex-no-such-package' \
    | write_workflow escaped-command-name ci.yml
expect "an escaped command name is still apt" 1 escaped-command-name \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...and the same question, asked where the line cannot be lexed at all. The
# untokenisable-line notice was the last test left reading raw source text, so
# `a\pt-get install -y 'unterminated` was skipped in silence — the very
# combination the token prefilter above exists to prevent.
printf '%s\n' \
    '        run: |' \
    '          sudo apt-get install -y cmake' \
    "          sudo a\\pt-get install -y 'unterminated" \
    | write_workflow escaped-name-unlexable ci.yml
expect "an unlexable line with an escaped name is announced" 0 escaped-name-unlexable \
    "could not be tokenised" "All 1 apt package(s)"

# ------------------------------------ a quoted digit is not an IO number
# `2>out` redirects; `'2'>out` passes `2` as an argument, because a QUOTED word
# is never an IO number. The adjacency test alone is not enough.
write_workflow fd-quoted ci.yml <<'YAML'
        run: sudo apt-get install -y cmake '2'>out
YAML
expect "a quoted digit is an argument, not a descriptor" 1 fd-quoted \
    "installs a package apt cannot resolve"

# ------------------------------------- only values that can carry shell
# A step's `name:` is metadata the runner never executes. Reading every scalar
# in the file made `name: apt-get install dependencies` an invocation, and the
# gate rejected the workflow over a package nobody installs — a false red on a
# file that is perfectly correct.
write_workflow step-name ci.yml <<'YAML'
jobs:
  build:
    steps:
      - name: apt-get install dependencies
        run: sudo apt-get install -y cmake
YAML
expect "a step name is not a command" 0 step-name \
    "All 1 apt package(s)"

# ...and the matrix value above still is one, because a `run:` refers to it.
# Narrowing this is exactly how the first library-based scanner lost five
# packages, so both halves stay pinned.
write_workflow metadata-vs-matrix ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        include:
          - os: linux
            extra_deps: sudo apt-get install -y libfcitx5-dev
    steps:
      - name: apt-get install dependencies
        run: ${{ matrix.extra_deps }}
YAML
expect "a value a run: refers to is still scanned" 1 metadata-vs-matrix \
    "libfcitx5-dev"


# ------------------------------------------- references written with brackets
# `${{ matrix['extra_deps'] }}` selects the same value as `matrix.extra_deps`.
# Splitting on dots alone records the whole `matrix['extra_deps']` as the name,
# so the referenced value is never scanned and its packages go unexamined and
# unmentioned.
write_workflow matrix-bracket ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        include:
          - extra_deps: sudo apt-get install -y codex-no-such-package
    steps:
      - run: sudo apt-get install -y cmake
      - run: ${{ matrix['extra_deps'] }}
YAML
expect "a bracketed reference is followed too" 1 matrix-bracket \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...but only for contexts that can carry a command. `github.event.repository.name`
# ends in `name` as well, and putting THAT in scope would drag every step's
# `name:` back in and reject the workflow all over again.
write_workflow metadata-reference ci.yml <<'YAML'
jobs:
  build:
    steps:
      - name: apt-get install dependencies
        run: echo ${{ github.event.repository.name }}
      - run: sudo apt-get install -y cmake
YAML
expect "a metadata reference brings nothing into scope" 0 metadata-reference \
    "All 1 apt package(s)"

# ------------------------------------------------- `<<` is also a left shift
# Inside `(( ))` bash reads `<<` as arithmetic, not as a heredoc. Opening one
# there swallows every command after it as data.
write_workflow arithmetic-shift ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          (( value = 1 << 2 ))
          sudo apt-get install -y codex-no-such-package
YAML
expect "a left shift is not a heredoc" 1 arithmetic-shift \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...and the arithmetic ENDS. A `<<` after `(( ))` on the same line is a
# heredoc again, so a depth that only ever grows leaves the body being read as
# commands.
write_workflow arithmetic-then-heredoc ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          (( value = 1 )) ; cat > x.sh <<'EOF'
          sudo apt-get install -y codex-no-such-package
          EOF
YAML
expect "arithmetic ends before the next heredoc" 0 arithmetic-then-heredoc \
    "All 1 apt package(s)" "inside a heredoc"

# ------------------------------------------------------------------------ done
printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
[ "$FAILED" -eq 0 ]
