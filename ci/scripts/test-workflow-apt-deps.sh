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
#
# A body that is not already a workflow document is WRAPPED in the steps
# structure a real workflow has. Most cases here are a single `run:` and read
# far better without the scaffolding — but a `run:` is only a script when it
# belongs to a step, so an unwrapped fragment tests a shape GitHub never
# produces, and a rule that accepted it would be a rule written for the
# fixtures rather than for workflows.
write_workflow() {
    mkdir -p "$WORK/$1"
    local body first
    body="$(cat)"
    first="$(printf '%s\n' "$body" | grep -m1 -v '^[[:space:]]*$' | sed 's/^[[:space:]]*//')"
    case "$first" in
    -*) printf 'jobs:\n  build:\n    steps:\n%s\n' "$body" >"$WORK/$1/$2" ;;
    run:* | name:* | env:* | uses:* | with:* | if:*)
        printf 'jobs:\n  build:\n    steps:\n      - name: fixture\n%s\n' "$body" >"$WORK/$1/$2" ;;
    *) printf '%s\n' "$body" >"$WORK/$1/$2" ;;
    esac
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
    # A needle written `!text` must be ABSENT. Without that, a case can only
    # say what the gate reported and never that it kept quiet — which is how
    # two mutations that made the scanner announce things it should not both
    # survived a battery.
    local needle
    for needle in "$@"; do
        if [ "${needle#!}" != "$needle" ]; then
            if printf '%s' "$out" | grep -qF -- "${needle#!}"; then
                fail "$label: output should not mention '${needle#!}'"
                printf '%s\n' "$out" | sed 's/^/    | /' >&2
                return
            fi
        elif ! printf '%s' "$out" | grep -qF -- "$needle"; then
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
    "libfcitx5-dev" "libfcitx5qt-dev" "libfcitx5qt1-dev" "release-main.yml:7"

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
    "libfcitx5-dev" "ci.yml:6"

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
    "libfcitx5-dev" "ci.yml:7"

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
    "libfcitx5-dev" "ci.yml:7"

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
# Nested as the real file nests it, under `strategy.matrix.include`, because a
# referenced name is scoped to its CONTEXT: a bare fragment with no `matrix`
# ancestor is not what this claims to mirror, and would pass while proving
# nothing about the workflow it stands for.
write_workflow matrix-value ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        include:
          - os: linux
            extra_deps: |
              sudo apt-get update
              sudo apt-get install -y libfcitx5-dev
    steps:
      - run: ${{ matrix.extra_deps }}
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
    # A needle written `!text` must be ABSENT. Without that, a case can only
    # say what the gate reported and never that it kept quiet — which is how
    # two mutations that made the scanner announce things it should not both
    # survived a battery.
    local needle
    for needle in "$@"; do
        if [ "${needle#!}" != "$needle" ]; then
            if printf '%s' "$out" | grep -qF -- "${needle#!}"; then
                fail "$label: output should not mention '${needle#!}'"
                printf '%s\n' "$out" | sed 's/^/    | /' >&2
                return
            fi
        elif ! printf '%s' "$out" | grep -qF -- "$needle"; then
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
# The result is the WORD bash builds — `cma\`, a newline, `ke` — which apt
# rejects, rather than the `cmake` a backslash continuation would have produced.
# This case used to expect the weaker "could not be tokenised" notice, because
# the scanner gave up at the end of the first physical line; carrying the open
# quote across the newline lets it name the package apt will refuse instead.
expect "a backslash inside single quotes continues nothing" 1 backslash-in-single-quotes \
    "installs a package apt cannot resolve"

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


# ------------------------------------------ a quote may span physical lines
# Bash carries an open quote across the newline, so `'codex-` and
# `no-such-package'` are ONE argument holding a newline — a name apt rejects.
# Scanning each physical line alone reduced the install to a non-failing
# "could not be tokenised" notice and ignored the closing fragment entirely.
write_workflow quote-across-lines ci.yml <<'YAML'
        run: |
          sudo apt-get install -y libfcitx5core-dev
          sudo apt-get install -y cmake 'codex-
          no-such-package'
YAML
expect "an open quote carries to the next line" 1 quote-across-lines \
    "installs a package apt cannot resolve"

# ...and the newline is KEPT, which is the whole difference. `'cma` + `ke'` is
# `cma`, a newline, `ke` — a name apt rejects — where joining the fragments
# without it would spell the perfectly valid `cmake` and pass the gate.
write_workflow quote-across-lines-newline ci.yml <<'YAML'
        run: |
          sudo apt-get install -y libfcitx5core-dev
          sudo apt-get install -y 'cma
          ke'
YAML
expect "the newline inside a carried quote is kept" 1 quote-across-lines-newline \
    "installs a package apt cannot resolve"

# The predicate behind that, pinned directly. A line ending in a lone backslash
# also stops the lexer, and reporting THAT as an open quote would join lines the
# shell keeps apart. The guard is unreachable from `scan_shell` — the backslash
# branch runs first — so it is asserted here rather than through a fixture.
if SCRIPT_DIR="$SCRIPT_DIR" python3 - <<'PYPIN'
import os
import sys
sys.path.insert(0, os.environ["SCRIPT_DIR"])
from workflow_apt_deps import ends_inside_a_quote

for text, want in [
    ("echo 'x", "'"),
    ('echo "x', '"'),
    ("echo x", None),
    ("echo x\\", None),
    ("echo 'x' y", None),
]:
    got = ends_inside_a_quote(text)
    if got != want:
        print(f"{text!r}: expected {want!r}, got {got!r}")
        raise SystemExit(1)
PYPIN
then
    ok "only an open quote carries a line, not a dangling escape"
else
    fail "only an open quote carries a line, not a dangling escape"
fi


# --------------------------------------- a quoted dot breaks a brace RANGE
# `codex{1".".2}` is `codex{1..2}` with one dot quoted, and bash expands
# nothing: the range separator is no longer literal, so apt is handed the whole
# thing and rejects it. Tracking quoting for braces and commas but not for the
# dots leaves it announced as dynamic and the broken install passes.
write_workflow brace-quoted-dot ci.yml <<'YAML'
        run: sudo apt-get install -y cmake codex{1".".2}
YAML
expect "a quoted dot stops a range expanding" 1 brace-quoted-dot \
    "installs a package apt cannot resolve"

# ...and an unquoted range still expands, so it stays unresolvable here.
write_workflow brace-range ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install -y codex{1..2}
YAML
expect "an unquoted range is a brace expansion" 0 brace-range \
    "All 1 apt package(s)" "not checked"

# ------------------------------------ every reference in a compound expression
# `${{ matrix.primary || matrix.fallback }}` may execute either value, so both
# have to be scanned. Taking only the LAST property access left `primary`
# unexamined and unmentioned.
write_workflow compound-reference ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        include:
          - primary: sudo apt-get install -y codex-no-such-package
            fallback: sudo apt-get install -y cmake
    steps:
      - run: ${{ matrix.primary || matrix.fallback }}
YAML
expect "both halves of a compound reference are scanned" 1 compound-reference \
    "codex-no-such-package" "installs a package apt cannot resolve"

# --------------------------------------- a referenced name is scoped to its context
# `${{ env.command }}` puts the ENV value in scope, not every scalar in the file
# whose key happens to be `command`. An action input under `with:` is never
# executed as shell, and resolving it rejects a workflow that works.
write_workflow reference-scope ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y cmake
    steps:
      - run: ${{ env.command }}
      - uses: some/action
        with:
          command: sudo apt-get install -y codex-no-such-package
YAML
expect "a referenced name is scoped to its context" 0 reference-scope \
    "All 1 apt package(s)"

# ...and the value it really does name is still scanned.
write_workflow reference-scope-hit ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y libfcitx5-dev
    steps:
      - run: ${{ env.command }}
YAML
expect "the value in the named context is scanned" 1 reference-scope-hit \
    "libfcitx5-dev"


# ------------------------------------------- a reference names a value IN SCOPE
# Two jobs may each define `env.command`. A `run:` in one of them names its own,
# not every value in the file with that key — matching names across the whole
# document rejects a workflow because of a string another job never executes.
write_workflow reference-other-job ci.yml <<'YAML'
jobs:
  first:
    env:
      command: sudo apt-get install -y codex-no-such-package
    steps:
      - run: echo not-executed
  second:
    env:
      command: sudo apt-get install -y cmake
    steps:
      - run: ${{ env.command }}
YAML
expect "a reference does not reach another job" 0 reference-other-job \
    "All 1 apt package(s)"

# A workflow input's value lives under `default:`, one level below the name the
# expression uses. An invocation that omits the input runs exactly that string.
write_workflow input-default ci.yml <<'YAML'
on:
  workflow_dispatch:
    inputs:
      command:
        default: sudo apt-get install -y codex-no-such-package
jobs:
  build:
    steps:
      - run: ${{ inputs.command }}
YAML
expect "an input default is the command that runs" 1 input-default \
    "codex-no-such-package" "installs a package apt cannot resolve"

# `${{ 'env.command' }}` is a string LITERAL, not a dereference. Reading the
# expression as raw text made it one, and the value it named was checked and
# the workflow rejected over a command nothing executes.
write_workflow expression-string ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y codex-no-such-package
    steps:
      - run: echo "${{ 'env.command' }}"
      - run: sudo apt-get install -y cmake
YAML
expect "a quoted expression string names nothing" 0 expression-string \
    "All 1 apt package(s)"

# ------------------------------------------------- the end-of-options marker
# After `--` a dash-prefixed word is a package operand, not an option. apt
# agrees: `apt-get -s install cmake -- -codex-no-such-package` exits 100 with
# "Unable to locate package -codex-no-such-package".
write_workflow end-of-options ci.yml <<'YAML'
        run: sudo apt-get install -y cmake -- -codex-no-such-package
YAML
expect "a dash after -- is a package name" 1 end-of-options \
    "installs a package apt cannot resolve"

# ...and before it, options are still options.
write_workflow options-before-marker ci.yml <<'YAML'
        run: |
          sudo apt-get install -y cmake
          sudo apt-get install --no-install-recommends -y cmake
YAML
expect "options before the marker are still options" 0 options-before-marker \
    "All 2 apt package(s)"


# ...and the marker belongs to the command that carried it. Left standing, the
# next command's options are read as package names and a correct workflow is
# rejected.
write_workflow marker-then-command ci.yml <<'YAML'
        run: sudo apt-get install -y cmake -- libfcitx5core-dev; sudo apt-get install --no-install-recommends -y cmake
YAML
expect "the end-of-options marker ends with its command" 0 marker-then-command \
    "All 3 apt package(s)"

# The same scoping for a matrix as for env: two jobs may each define
# `extra_deps`, and only the job that references it runs it.
write_workflow matrix-other-job ci.yml <<'YAML'
jobs:
  first:
    strategy:
      matrix:
        include:
          - extra_deps: sudo apt-get install -y codex-no-such-package
    steps:
      - run: echo not-executed
  second:
    strategy:
      matrix:
        include:
          - extra_deps: sudo apt-get install -y cmake
    steps:
      - run: ${{ matrix.extra_deps }}
YAML
expect "a matrix reference does not reach another job" 0 matrix-other-job \
    "All 1 apt package(s)"


# --------------------------------------------- env precedence, step by step
# Two steps in one job may each set `env.command`, and a `run:` sees its OWN.
# Selecting every match in the job scans a value the referencing step overrode
# and rejects a workflow that works.
write_workflow env-step-precedence ci.yml <<'YAML'
jobs:
  build:
    steps:
      - env:
          command: sudo apt-get install -y codex-no-such-package
        run: echo not-executed
      - env:
          command: sudo apt-get install -y cmake
        run: ${{ env.command }}
YAML
expect "a step sees its own env, not its neighbour's" 0 env-step-precedence \
    "All 1 apt package(s)"

# ...and a step's env overrides the job's, which is the same rule one level up.
write_workflow env-step-over-job ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y codex-no-such-package
    steps:
      - env:
          command: sudo apt-get install -y cmake
        run: ${{ env.command }}
YAML
expect "a step's env overrides the job's" 0 env-step-over-job \
    "All 1 apt package(s)"

# ...while the job's is used when the step sets none.
write_workflow env-job-fallback ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y libfcitx5-dev
    steps:
      - run: ${{ env.command }}
YAML
expect "a job's env is used when the step sets none" 1 env-job-fallback \
    "libfcitx5-dev"

# ------------------------------------- commands assembled at run time
# `${{ format('sudo apt-get install -y {0}', matrix.package) }}` builds the
# command itself: the `run:` text holds no apt invocation and the matrix value
# is a bare word, so nothing was checked AND nothing was said. This scanner
# announces what it cannot resolve — that is the whole contract — so an
# assembled command is announced.
write_workflow assembled-command ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        include:
          - package: codex-no-such-package
    steps:
      - run: sudo apt-get install -y cmake
      - run: ${{ format('sudo apt-get install -y {0}', matrix.package) }}
YAML
expect "a command assembled by an expression is announced" 0 assembled-command \
    "All 1 apt package(s)" "assembled at run time"

# ...but a plain reference is not "assembled": its value IS the command, and it
# is scanned rather than excused.
write_workflow plain-reference-not-assembled ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        include:
          - extra_deps: sudo apt-get install -y libfcitx5-dev
    steps:
      - run: ${{ matrix.extra_deps }}
YAML
expect "a plain reference is scanned, not announced" 1 plain-reference-not-assembled \
    "libfcitx5-dev" '!assembled at run time'

# ...and an expression INSIDE an ordinary script is not an assembled command.
# `echo ${{ github.ref }}` is a script with a value interpolated into it, and
# its apt lines are read as usual — announcing those would bury the real
# notices under one per metadata reference, which is what the first version of
# this rule did to nineteen lines of this repository's own workflows.
write_workflow interpolation-in-script ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          echo ${{ github.ref }}
          sudo apt-get install -y cmake
YAML
expect "an interpolation inside a script is not an assembled command" 0 interpolation-in-script \
    "All 1 apt package(s)" '!assembled at run time'


# ------------------------------- env is inherited, not shared between siblings
# A step sees its own env, then its job's, then the workflow's. It never sees
# another STEP's. Ranking candidates by how much path they share picks a
# sibling — which shares `jobs/build/steps` — ahead of the job value the step
# actually inherits.
write_workflow env-sibling-step ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y cmake
    steps:
      - env:
          command: sudo apt-get install -y codex-no-such-package
        run: echo not-executed
      - run: ${{ env.command }}
YAML
expect "a sibling step's env is not inherited" 0 env-sibling-step \
    "All 1 apt package(s)"

# ------------------------------------- an interpolation may be DATA, not command
# `echo "install with ${{ env.help }}"` prints that text and runs nothing. The
# value is only the command when the expression IS the command — otherwise
# following it rejects a workflow over a string it merely echoes.
write_workflow interpolation-as-data ci.yml <<'YAML'
jobs:
  build:
    env:
      help: sudo apt-get install -y codex-no-such-package
    steps:
      - run: echo "install with ${{ env.help }}"
      - run: sudo apt-get install -y cmake
YAML
expect "an interpolated value used as data is not a command" 0 interpolation-as-data \
    "All 1 apt package(s)"

# ------------------------------------ an expression inside the command NAME
# `a${{ '' }}pt-get` runs apt-get, but the masked word is neither apt nor a
# bare expression, so the command was neither checked NOR announced. A word
# part literal and part expression is assembled at run time like any other.
write_workflow assembled-command-name ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: a${{ '' }}pt-get install -y codex-no-such-package
      - run: sudo apt-get install -y cmake
YAML
expect "an expression inside a command name is announced" 0 assembled-command-name \
    "All 1 apt package(s)" "assembled at run time"

# ...while an expression that is the WHOLE word is a value reference and is
# followed, not announced as an assembled name.
write_workflow whole-word-expression ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        include:
          - extra_deps: sudo apt-get install -y libfcitx5-dev
    steps:
      - run: ${{ matrix.extra_deps }}
YAML
expect "a whole-word expression is a reference, not an assembled name" 1 whole-word-expression \
    "libfcitx5-dev" '!assembled at run time'

# ...and an ASSIGNMENT names no command. `TAG=${{ inputs.tag }}` sets a
# variable, and announcing those flagged two lines of this repository's own
# workflows when the check ran before the assignment clause.
write_workflow assignment-expression ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          TAG=${{ inputs.tag }}
          sudo apt-get install -y cmake
YAML
expect "an assignment holding an expression names no command" 0 assignment-expression \
    "All 1 apt package(s)" '!assembled at run time'


# ------------------------------- an expression may be one line of a script
# A `run:` block may hold ordinary lines AND a line that is nothing but an
# expression. Deciding once for the whole scalar means the second is never
# followed: `echo preparing` makes the block "written here" and the command on
# the next line is neither checked nor announced.
write_workflow expression-own-line ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y codex-no-such-package
    steps:
      - run: |
          echo preparing
          ${{ env.command }}
YAML
expect "an expression on its own line is still a command" 1 expression-own-line \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...and a line with an expression IN it is still ordinary script, per line.
write_workflow expression-in-line ci.yml <<'YAML'
jobs:
  build:
    env:
      help: sudo apt-get install -y codex-no-such-package
    steps:
      - run: |
          echo "install with ${{ env.help }}"
          sudo apt-get install -y cmake
YAML
expect "an expression inside a line is still data" 0 expression-in-line \
    "All 1 apt package(s)"

# ------------------------------------------ a value named `run` is not a step
# `env: {run: ...}` is an environment variable that GitHub exports and never
# executes. Accepting every terminal key called `run` rejected a valid
# workflow over a string nothing runs.
write_workflow env-named-run ci.yml <<'YAML'
env:
  run: apt-get install -y codex-no-such-package
jobs:
  build:
    steps:
      - run: sudo apt-get install -y cmake
YAML
expect "a variable named run is not a step's script" 0 env-named-run \
    "All 1 apt package(s)"

# ...and an action input called `run` is not one either.
write_workflow with-named-run ci.yml <<'YAML'
jobs:
  build:
    steps:
      - uses: some/action
        with:
          run: apt-get install -y codex-no-such-package
      - run: sudo apt-get install -y cmake
YAML
expect "an action input named run is not a step's script" 0 with-named-run \
    "All 1 apt package(s)"


# --------------------------- an expression may be a command after a separator
# `echo preparing; ${{ env.command }}` runs the value after the semicolon.
# Asking the question per LINE was one granularity short: the line holds
# ordinary text, but the SEGMENT after the separator is nothing but the
# expression.
write_workflow expression-after-separator ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y codex-no-such-package
    steps:
      - run: echo preparing; ${{ env.command }}
YAML
expect "an expression after a separator is a command" 1 expression-after-separator \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...and a segment with an expression IN it is still data.
write_workflow expression-in-segment ci.yml <<'YAML'
jobs:
  build:
    env:
      help: sudo apt-get install -y codex-no-such-package
    steps:
      - run: echo preparing; echo "install with ${{ env.help }}"
      - run: sudo apt-get install -y cmake
YAML
expect "an expression inside a segment is still data" 0 expression-in-segment \
    "All 1 apt package(s)"

# ...and the segments come from the LEXER, so a quoted `;` stays part of its
# word. Here the command is the referenced value plus a literal `;` argument —
# something IS written on that segment, so the value is not the command and is
# not followed. Splitting on the quoted separator would leave the expression
# alone in a segment and follow it.
write_workflow quoted-separator-segment ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y codex-no-such-package
    steps:
      - run: echo x; echo ';' ${{ env.command }}
      - run: sudo apt-get install -y cmake
YAML
# Two things are load-bearing here. The leading `echo x;` gives the line a real
# separator, without which it is never segmented at all; and the `echo` before
# the quoted `;` means the second segment holds a command of its own, so the
# value is an argument to it and not the command. Split on the QUOTED
# separator as well and the expression would stand alone in a segment and be
# followed as a command.
expect "a quoted separator does not split a segment" 0 quoted-separator-segment \
    "All 1 apt package(s)"

# ---------------------------------- only a STEP's run: is a step's script
# A job output named `run` is defined, not executed. Naming the contexts that
# are NOT scripts is a list that grows one review round at a time — `env`,
# then `with`, now `outputs` — so the rule is the other way round: a `run:` is
# a script when it belongs to a step.
write_workflow outputs-named-run ci.yml <<'YAML'
jobs:
  build:
    outputs:
      run: apt-get install -y codex-no-such-package
    steps:
      - run: sudo apt-get install -y cmake
YAML
expect "a job output named run is not a step's script" 0 outputs-named-run \
    "All 1 apt package(s)"


# --------------------------------- the WHOLE step path, not its last three keys
# A matrix dimension may be called `steps` and hold objects with a `run:` key.
# Checking only the last three components matches that as readily as a real
# step, and rejects a workflow over matrix DATA.
write_workflow matrix-named-steps ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        steps:
          - run: apt-get install -y codex-no-such-package
    steps:
      - run: sudo apt-get install -y cmake
YAML
expect "a matrix dimension named steps is not a step" 0 matrix-named-steps \
    "All 1 apt package(s)"

# ------------------------------- a command may be preceded by shell prefixes
# `FLAG=1 ${{ env.command }}` runs the value with a variable set for it. The
# assignment is not the command, so the expression still supplies one — and
# treating the prefix as "something written here" skipped the reference.
write_workflow assignment-prefix-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y codex-no-such-package
    steps:
      - run: FLAG=1 ${{ env.command }}
YAML
expect "an assignment prefix does not hide the command" 1 assignment-prefix-reference \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...and a redirection before it is a prefix too.
write_workflow redirect-prefix-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y codex-no-such-package
    steps:
      - run: 2>/dev/null ${{ env.command }}
YAML
expect "a redirection prefix does not hide the command" 1 redirect-prefix-reference \
    "codex-no-such-package"

# ...while real text before it still means the value is data.
write_workflow text-prefix-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      help: sudo apt-get install -y codex-no-such-package
    steps:
      - run: echo ${{ env.help }}
      - run: sudo apt-get install -y cmake
YAML
expect "text before an expression still makes it data" 0 text-prefix-reference \
    "All 1 apt package(s)"

# ...and a segment of nothing but assignments IS a command: `OUT=x.apk` sets a
# variable and runs nothing else. Reading it as "no command written here"
# announced two lines of this repository's own android-release.yml.
write_workflow assignment-only-segment ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          OUT="idiolect-${{ github.sha }}.apk"
          sudo apt-get install -y cmake
YAML
expect "an assignment on its own is a command" 0 assignment-only-segment \
    "All 1 apt package(s)" '!assembled at run time'

# ...and a QUOTED expression is a literal argument, not a command being
# supplied. `[[ "${{ github.ref }}" == refs/tags/* ]]` is a test — and the
# quoting only survives if segments keep their WORDS rather than being rebuilt
# as strings, which is how this reached the real workflow.
write_workflow quoted-expression-operand ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          if [[ "${{ github.event_name }}" == "push" && "${{ github.ref }}" == refs/tags/* ]]; then echo tagged; fi
          sudo apt-get install -y cmake
YAML
expect "a quoted expression is an argument, not a command" 0 quoted-expression-operand \
    "All 1 apt package(s)" '!assembled at run time'

# ------------------------------------------ ANSI-C quoting decodes escapes
# `$'c\x6dake'` is bash's ANSI-C quoting and the package it installs is
# `cmake`. Removing the dollar without decoding leaves a literal `c\x6dake`,
# which apt cannot resolve — a false red on a workflow that works.
write_workflow ansi-c-escape ci.yml <<'YAML'
        run: sudo apt-get install -y $'c\x6dake'
YAML
expect "ANSI-C quoting decodes its escapes" 0 ansi-c-escape \
    "All 1 apt package(s)"

# ...and a shell control word is a prefix too: `if ${{ env.command }}; then`
# RUNS the value, so the reference still has to be followed.
write_workflow control-prefix-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y codex-no-such-package
    steps:
      - run: if ${{ env.command }}; then :; fi
YAML
expect "a control word does not hide the command" 1 control-prefix-reference \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...as is the negation that can precede a pipeline. Written as a block scalar
# because a plain one starting `!` is a YAML TAG: `run: ! ${{ ... }}` reaches
# the shell with the bang already eaten, and the fixture tests nothing.
write_workflow negation-prefix-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      command: sudo apt-get install -y codex-no-such-package
    steps:
      - run: |
          ! ${{ env.command }}
YAML
expect "a negation does not hide the command" 1 negation-prefix-reference \
    "codex-no-such-package"

# ...and so is the sudo that already prefixes every apt line in this repository.
write_workflow sudo-prefix-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      command: apt-get install -y codex-no-such-package
    steps:
      - run: sudo ${{ env.command }}
YAML
expect "sudo does not hide the command" 1 sudo-prefix-reference \
    "codex-no-such-package"

# ...and an option belongs to the prefix that takes it: `sudo -E` is the
# documented form this repository's own comment cites, and the command after it
# is still the value's.
write_workflow sudo-option-prefix-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      command: apt-get install -y codex-no-such-package
    steps:
      - run: sudo -E ${{ env.command }}
YAML
expect "an option after sudo does not hide the command" 1 sudo-option-prefix-reference \
    "codex-no-such-package"

# ...while an option with nothing in front of it is not prefix material: only a
# prefix command can own one, and reading a bare `-x` as one would follow a
# value no command runs.
write_workflow bare-option-not-a-prefix ci.yml <<'YAML'
jobs:
  build:
    env:
      help: sudo apt-get install -y codex-no-such-package
    steps:
      - run: -x ${{ env.help }}
      - run: sudo apt-get install -y cmake
YAML
expect "a bare option is not an invocation prefix" 0 bare-option-not-a-prefix \
    "All 1 apt package(s)"

# ...while a control word used as an ARGUMENT is still ordinary text, and the
# value after it stays data.
write_workflow control-word-argument ci.yml <<'YAML'
jobs:
  build:
    env:
      help: sudo apt-get install -y codex-no-such-package
    steps:
      - run: echo if ${{ env.help }}
      - run: sudo apt-get install -y cmake
YAML
expect "a control word mid-command does not follow the value" 0 control-word-argument \
    "All 1 apt package(s)"

# --------------------------------------- ANSI-C quoting escapes its own quote
# `$'a\'b'` is one word: inside ANSI-C quoting a backslash escapes the quote,
# so the string does not end there. shlex has no such rule and gives up on the
# apostrophe left over — which turned an unresolvable package into a notice
# nobody fails on.
write_workflow ansi-c-escaped-quote ci.yml <<'YAML'
        run: sudo apt-get install -y $'codex-no-\'such-package'
YAML
expect "ANSI-C quoting escapes its own quote" 1 ansi-c-escaped-quote \
    "codex-no-'such-package"

# ...and once shlex has left the string at that escaped quote, a `$` before the
# real closing quote looks to it like ANOTHER `$'` opening — which would move
# the span's start past the name and lose it again.
write_workflow ansi-c-escaped-quote-dollar ci.yml <<'YAML'
        run: sudo apt-get install -y $'codex-no-\'such$'
YAML
expect "a dollar before the closing quote does not restart the span" 1 ansi-c-escaped-quote-dollar \
    "codex-no-'such\$"

# ...and a QUOTED reserved word is not one: bash looks for a command called
# `if` and finds none, so what follows is its argument rather than a command.
write_workflow control-word-quoted ci.yml <<'YAML'
jobs:
  build:
    env:
      help: sudo apt-get install -y codex-no-such-package
    steps:
      - run: |
          "if" ${{ env.help }}
      - run: sudo apt-get install -y cmake
YAML
expect "a quoted control word is not a prefix" 0 control-word-quoted \
    "All 1 apt package(s)"

# ...and the backslashes come in PAIRS, as everywhere else: `$'a\\'` holds one
# literal backslash and the quote after it closes the string, so reading the
# second backslash as an escape would swallow the closing quote.
write_workflow ansi-c-double-backslash ci.yml <<'YAML'
        run: sudo apt-get install -y $'codex-no\\'
YAML
expect "backslashes inside ANSI-C quoting pair up" 1 ansi-c-double-backslash \
    'codex-no\' "installs a package apt cannot resolve"

# ...and the OTHER form escapes its quote too, by shlex's rule rather than
# bash's: `$"..."` is a translation, and shlex reads double-quoted escapes
# correctly, so the span must close on the quote shlex says closes it.
write_workflow dollar-double-quote-escaped ci.yml <<'YAML'
        run: sudo apt-get install -y $"codex-no\"such-package"
YAML
expect 'a $"..." span closes where shlex says it does' 1 dollar-double-quote-escaped \
    'codex-no"such-package'

# ---------------------------------- a control word is a command position too
# The expression path learned that `if` introduces a command; the scanner that
# reads LITERAL commands had not. `if apt-get install -y bad; then` produced a
# "not parsed" notice — which does not fail — so the package went unchecked.
write_workflow control-word-literal-command ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          if apt-get install -y codex-no-such-package; then :; fi
          sudo apt-get install -y cmake
YAML
expect "a control word still leaves a command position" 1 control-word-literal-command \
    "codex-no-such-package" "installs a package apt cannot resolve" \
    '!looks like an apt install command but was not parsed'

# ...and so does a subshell.
write_workflow subshell-literal-command ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          ( apt-get install -y codex-no-such-package )
          sudo apt-get install -y cmake
YAML
expect "a subshell still leaves a command position" 1 subshell-literal-command \
    "codex-no-such-package"

# ...and a brace group.
write_workflow group-literal-command ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          { apt-get install -y codex-no-such-package; }
          sudo apt-get install -y cmake
YAML
expect "a brace group still leaves a command position" 1 group-literal-command \
    "codex-no-such-package"

# ...while QUOTED it is not reserved at all: bash looks for a command called
# `if`, does not find one, and never runs the install — so checking its
# packages would be a red on something that cannot happen, and the notice is
# the honest answer.
write_workflow quoted-control-word-literal ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          "if" apt-get install -y codex-no-such-package
          sudo apt-get install -y cmake
YAML
expect "a quoted control word opens no command position" 0 quoted-control-word-literal \
    "looks like an apt install command but was not parsed" '!cannot resolve'

# --------------------------- an expression stands where its LOGICAL line does
# `echo \` continued onto `${{ env.command }}` passes the value to echo as
# arguments; bash runs no command from it. Reading the second PHYSICAL line as
# a segment of its own made the expression look like a command being supplied,
# and a workflow that never runs apt was rejected for a package inside a
# variable it only ever echoes.
write_workflow continuation-before-expression ci.yml <<'YAML'
jobs:
  build:
    env:
      command: apt-get install -y codex-no-such-package
    steps:
      - run: |
          echo \
            ${{ env.command }}
          sudo apt-get install -y cmake
YAML
expect "a continued line is one command" 0 continuation-before-expression \
    "All 1 apt package(s)" '!codex-no-such-package'

# ...and a heredoc BODY is data by the same argument: the shell runs none of
# it, so an expression in one supplies no command either.
write_workflow heredoc-body-expression ci.yml <<'YAML'
jobs:
  build:
    env:
      command: apt-get install -y codex-no-such-package
    steps:
      - run: |
          cat <<'EOF'
          ${{ env.command }}
          EOF
          sudo apt-get install -y cmake
YAML
expect "an expression in a heredoc body is data" 0 heredoc-body-expression \
    "All 1 apt package(s)" '!codex-no-such-package'

# ------------------------------------------- a matrix dimension is usually a LIST
# `matrix.command: [a, b]` runs a job per entry, and each entry's own path ends
# in its INDEX rather than the dimension's name — so a reference to it selected
# nothing at all, and the install inside went neither checked nor announced.
write_workflow matrix-list-command ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        command:
          - apt-get install -y cmake
          - apt-get install -y codex-no-such-package
    steps:
      - run: ${{ matrix.command }}
YAML
expect "a list-valued matrix dimension resolves" 1 matrix-list-command \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...but an entry's KEYS are not the dimension: a dimension whose entries are
# objects is referenced as `matrix.target.command`, and `${{ matrix.target }}`
# interpolates the object itself, which runs no install. Matching the name
# anywhere in the path would follow the value inside it and reject a workflow
# for a package apt is never asked for.
write_workflow matrix-object-entries ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        target:
          - os: ubuntu
            command: apt-get install -y codex-no-such-package
    steps:
      - run: ${{ matrix.target }}
      - run: sudo apt-get install -y cmake
YAML
expect "a matrix entry's keys are not the dimension" 0 matrix-object-entries \
    "All 1 apt package(s)" '!codex-no-such-package'

# --------------------------------------- a command may be more than one hop away
# A step's `env.COMMAND: ${{ matrix.install }}` run as `${{ env.COMMAND }}`
# reaches its apt line through TWO references. Stopping at the first scanned an
# env value that is itself only an expression, and the matrix entry holding the
# install went neither checked nor announced.
write_workflow chained-reference ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        install:
          - apt-get install -y codex-no-such-package
    steps:
      - env:
          COMMAND: ${{ matrix.install }}
        run: ${{ env.COMMAND }}
      - run: sudo apt-get install -y cmake
YAML
expect "a reference through a reference is followed" 1 chained-reference \
    "codex-no-such-package" "installs a package apt cannot resolve"

# ...and two values that name each other must not chase one another for ever.
# This case is here to TERMINATE; the packages it reports are beside the point.
write_workflow cyclic-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      A: ${{ env.B }}
      B: ${{ env.A }}
    steps:
      - run: ${{ env.A }}
      - run: sudo apt-get install -y cmake
YAML
expect "values naming each other terminate" 0 cyclic-reference \
    "All 1 apt package(s)"

# ...and a hop still ends where the command is WRITTEN: an env value that runs a
# command of its own with the next reference as an argument supplies nothing.
write_workflow chained-reference-argument ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        install:
          - apt-get install -y codex-no-such-package
    steps:
      - env:
          COMMAND: echo ${{ matrix.install }}
        run: ${{ env.COMMAND }}
      - run: sudo apt-get install -y cmake
YAML
expect "a hop stops where a command is written" 0 chained-reference-argument \
    "All 1 apt package(s)" '!codex-no-such-package'

# ------------------------------------------- a reference names a CHAIN, not a leaf
# `${{ matrix.target.command }}` names the `command` OF `target`. Collapsing it
# to the leaf matched every matrix value in the job ending in that name, so a
# second object-valued dimension with a `command` field — a help string nothing
# runs — was checked and rejected the workflow.
write_workflow matrix-property-chain ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        target:
          - command: apt-get install -y cmake
        metadata:
          - command: apt-get install -y codex-no-such-package
    steps:
      - run: ${{ matrix.target.command }}
YAML
expect "a property chain names one dimension" 0 matrix-property-chain \
    "All 1 apt package(s)" '!codex-no-such-package'

# ...while an include entry still reaches the run: that names it directly, which
# is the reason the leaf was being used in the first place.
write_workflow matrix-include-leaf ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        include:
          - extra_deps: apt-get install -y codex-no-such-package
    steps:
      - run: ${{ matrix.extra_deps }}
YAML
expect "an include entry is reached by its own name" 1 matrix-include-leaf \
    "codex-no-such-package"

# ---------------------------------- a command nobody can resolve is ANNOUNCED
# `${{ vars.COMMAND }}` is a repository setting: the value is not in the file,
# so the whole command is unknown. Resolving to nothing and saying nothing read
# exactly like a clean result.
write_workflow unresolvable-command ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: ${{ vars.COMMAND }}
      - run: sudo apt-get install -y cmake
YAML
expect "a command with no value in the file is announced" 0 unresolvable-command \
    "not checkable" "vars.COMMAND"

# --------------------------------------------------- a case arm runs its body
# `a) ${{ env.command }};;` executes the value: the pattern and its parenthesis
# are syntax. The pattern word made the segment look like a command written
# here, so the reference was never followed.
write_workflow case-arm-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      command: apt-get install -y codex-no-such-package
    steps:
      - run: |
          case "$x" in
          a) ${{ env.command }};;
          esac
          sudo apt-get install -y cmake
YAML
expect "a case arm runs its body" 1 case-arm-reference \
    "codex-no-such-package"

# ...while a subshell's closing parenthesis comes AFTER what runs, so it is not
# a pattern's and must not carry the walk past the expression.
write_workflow subshell-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      command: apt-get install -y codex-no-such-package
    steps:
      - run: |
          ( ${{ env.command }} )
          sudo apt-get install -y cmake
YAML
expect "a subshell still supplies its command" 1 subshell-reference \
    "codex-no-such-package"

# ------------------------------------------ a VIRTUAL package has no candidate
# `libz-dev` is provided by `zlib1g-dev` and nothing else, so `apt-get install`
# takes it while `apt-cache policy` reports `Candidate: (none)`. Reading that as
# "does not exist" is a red on a workflow that installs perfectly well.
write_workflow virtual-package ci.yml <<'YAML'
        run: sudo apt-get install -y libz-dev
YAML
expect "a uniquely provided virtual package resolves" 0 virtual-package \
    "All 1 apt package(s)"

# --------------------------------------------- `exclude:` names what does NOT run
# An entry under `exclude:` is a combination to DROP. Its own scalar is not a
# value, and when it names one dimension and nothing else the value it names
# runs in no combination at all — so scanning either rejected a workflow for a
# package apt is never asked for.
write_workflow matrix-excluded-value ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        command:
          - apt-get install -y cmake
          - apt-get install -y codex-no-such-package
        exclude:
          - command: apt-get install -y codex-no-such-package
    steps:
      - run: ${{ matrix.command }}
YAML
expect "an excluded matrix value is not run" 0 matrix-excluded-value \
    "All 1 apt package(s)" '!codex-no-such-package'

# ...but an exclusion of a COMBINATION removes only that combination: the value
# still runs everywhere the entry does not match, and skipping it would lose a
# package that really is installed.
write_workflow matrix-excluded-combination ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        os:
          - ubuntu
          - macos
        command:
          - apt-get install -y codex-no-such-package
        exclude:
          - os: macos
            command: apt-get install -y codex-no-such-package
    steps:
      - run: ${{ matrix.command }}
YAML
expect "excluding one combination keeps the value" 1 matrix-excluded-combination \
    "codex-no-such-package"

# ...and an entry under `exclude:` is not a value of anything, whether or not
# the dimension holds what it names. An exclusion that matches nothing — a
# typo, or a value since removed — must not become a package to check.
write_workflow matrix-exclude-entry-scanned ci.yml <<'YAML'
jobs:
  build:
    strategy:
      matrix:
        os:
          - ubuntu
          - macos
        command:
          - apt-get install -y cmake
        exclude:
          - os: macos
            command: apt-get install -y codex-no-such-package
    steps:
      - run: ${{ matrix.command }}
YAML
expect "an exclude entry is not a value" 0 matrix-exclude-entry-scanned \
    "All 1 apt package(s)" '!codex-no-such-package'

# ------------------------------------- the shell's own way of running a command
# `command apt-get …` and `exec apt-get …` both RUN apt — `command` bypasses
# functions and aliases, `exec` replaces the shell — so the install happens and
# the packages are the command's.
write_workflow command-builtin ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          command apt-get install -y codex-no-such-package
          sudo apt-get install -y cmake
YAML
expect "the command builtin runs its argument" 1 command-builtin \
    "codex-no-such-package" '!looks like an apt install command but was not parsed'

write_workflow exec-builtin ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          exec apt-get install -y codex-no-such-package
          sudo apt-get install -y cmake
YAML
expect "exec runs its argument" 1 exec-builtin \
    "codex-no-such-package"

# ------------------------------------ a case arm holds a command, literal or not
# The expression walk learned that `x) …` runs what follows; the scanner reading
# LITERAL commands had not, so a real install in an arm was announced as
# unparsed — a notice, which nothing fails on.
write_workflow case-arm-literal ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          case "$x" in
          x) apt-get install -y codex-no-such-package;;
          esac
          sudo apt-get install -y cmake
YAML
expect "a case arm's own command is checked" 1 case-arm-literal \
    "codex-no-such-package" '!looks like an apt install command but was not parsed'

# ...and the parenthesis of a function DEFINITION closes one, so it is not an
# arm's and opens no command position: the body runs when the function is
# called, not where it is written.
write_workflow function-definition-literal ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          install_deps() { apt-get install -y codex-no-such-package; }
          sudo apt-get install -y cmake
YAML
expect "a function definition is not a case arm" 0 function-definition-literal \
    "looks like an apt install command but was not parsed" '!cannot resolve'

# ------------------------------------- `name=(...)` is an ARRAY, not a subshell
# Reading the parenthesis as a command position — which is what made `( apt-get
# … )` work — turned an array initializer into an apt invocation whose last
# package was the closing bracket, and rejected a working workflow for a
# package called `)`.
write_workflow array-assignment ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          command=(apt-get install -y cmake)
          sudo "${command[@]}"
YAML
expect "an array initializer is not a subshell" 0 array-assignment \
    "All 1 apt package(s)" '!cannot resolve'

# ...and its elements ARE the command that gets run, so a name apt cannot
# resolve inside one is still caught — skipping the group outright would have
# hidden a real install behind a silent pass.
write_workflow array-assignment-bad ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          command=(apt-get install -y codex-no-such-package)
          sudo "${command[@]}"
YAML
expect "an array's elements are still checked" 1 array-assignment-bad \
    "codex-no-such-package" '!: )'

# ...and a reference among those elements is followed for the same reason: the
# array is what gets run, so its contents are read as the command whether they
# are written out or interpolated.
write_workflow array-assignment-reference ci.yml <<'YAML'
jobs:
  build:
    env:
      command: apt-get install -y codex-no-such-package
    steps:
      - run: |
          deps=(${{ env.command }})
          sudo "${deps[@]}"
YAML
expect "an array element is a command position" 1 array-assignment-reference \
    "codex-no-such-package"

# ------------------------------------ a command held in a SHELL variable is read
# `run: $COMMAND` runs whatever the variable holds, bash splitting it into
# words. When the step's own `env:` sets it, that value is in the file and is
# read like any other reference — reaching it only through `${{ }}` left a real
# install neither checked nor announced.
write_workflow shell-variable-command ci.yml <<'YAML'
jobs:
  build:
    steps:
      - env:
          COMMAND: sudo apt-get install -y codex-no-such-package
        run: $COMMAND
      - run: sudo apt-get install -y cmake
YAML
expect "a command from a shell variable is followed" 1 shell-variable-command \
    "codex-no-such-package"

# ...and one the workflow does not set is ordinary shell: nothing in the file
# says what it holds, and announcing every `$TOOL build` would bury the notices
# that mean something.
write_workflow shell-variable-unknown ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          $MAKE build
          sudo apt-get install -y cmake
YAML
expect "an unset shell variable says nothing" 0 shell-variable-unknown \
    "All 1 apt package(s)" '!not checkable'

# ...and a QUOTED one is a single word, so bash looks for a command whose name
# is the whole string, finds none, and installs nothing. Following it would red
# a workflow for a package that cannot be reached.
#
# Written as a block scalar because `run: "$COMMAND"` puts the quotes in YAML,
# not in the shell: the script would read `$COMMAND` bare and the case would
# test the opposite of what it says.
write_workflow shell-variable-quoted ci.yml <<'YAML'
jobs:
  build:
    steps:
      - env:
          COMMAND: sudo apt-get install -y codex-no-such-package
        run: |
          "$COMMAND"
      - run: sudo apt-get install -y cmake
YAML
expect "a quoted variable runs no command" 0 shell-variable-quoted \
    "All 1 apt package(s)" '!codex-no-such-package'

# --------------------------------------------- `bash -c` runs a script argument
# The quoted argument of `bash -c` is a script, and apt inside it installs for
# real. Stopping at `bash` left it neither scanned nor announced.
write_workflow shell-dash-c ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash -c 'sudo apt-get install -y codex-no-such-package'
      - run: sudo apt-get install -y cmake
YAML
expect "bash -c scans its script" 1 shell-dash-c \
    "codex-no-such-package"

# ...and a valid one inside is checked rather than merely announced.
write_workflow shell-dash-c-valid ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: sh -c 'apt-get install -y cmake'
YAML
expect "sh -c checks what it installs" 0 shell-dash-c-valid \
    "All 1 apt package(s)" '!not checkable'

# ------------------------------------- `inputs` is a workflow's, not a matrix's
# A matrix dimension may be CALLED inputs. Matching every path holding that word
# resolved a matrix field for `${{ inputs.command }}`, which reaches only the
# workflow's own declarations — and rejected the workflow for a value it never
# interpolates.
write_workflow inputs-vs-matrix ci.yml <<'YAML'
on:
  workflow_call:
    inputs:
      command:
        default: apt-get install -y cmake
jobs:
  build:
    strategy:
      matrix:
        inputs:
          - command: apt-get install -y codex-no-such-package
    steps:
      - run: ${{ inputs.command }}
YAML
expect "inputs names the workflow's own declarations" 0 inputs-vs-matrix \
    "All 1 apt package(s)" '!codex-no-such-package'

# ------------------------------------------------------------------------ done
printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
[ "$FAILED" -eq 0 ]
