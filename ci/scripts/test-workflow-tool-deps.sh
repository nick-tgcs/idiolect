#!/usr/bin/env bash
# Tests for check-workflow-tool-deps.sh — the workflow command-line-tool gate.
#
# The gate exists because a missing tool is not a loud failure here. Every check
# that uses ripgrep is written `if rg ...; then fail; fi`, so a `rg` that is not
# installed returns 127, the `if` reads that as "no matches", and the check
# reports a clean pass having examined nothing. Two CI jobs did exactly that:
# the develop run on 2026-09-05 printed `rg: command not found` from both
# test-interface-no-backend-leakage.sh and test-real-adapter-deps.sh, and went
# green.
#
# So the cases below pull in two directions. The gate must see a tool used
# through a chain of scripts it is not looking at, and it must not invent a
# dependency out of a word that merely contains the letters.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$SCRIPT_DIR/check-workflow-tool-deps.sh"

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

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

write_workflow() { # write_workflow <dir-name> <file-name>, body on stdin
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
    # A needle written `!text` must be ABSENT — a case that can only say what
    # the gate reported, never that it kept quiet, cannot catch a gate that
    # reports everything.
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
# ci.yml's interface-leakage job, as it stood: checkout, toolchain, cache, run
# the check. Nothing installs ripgrep, and the runner image does not ship it.
write_workflow real ci.yml <<'YAML'
jobs:
  interface-leakage:
    steps:
      - uses: actions/checkout@v7
      - name: Run interface leakage check
        run: bash ci/scripts/test-interface-no-backend-leakage.sh
YAML
expect "the historical interface-leakage job is rejected" 1 real \
    "interface-leakage" "ripgrep"

# The same job with the step that was missing. The fix has to satisfy the gate,
# or the gate is unfixable and gets switched off.
write_workflow fixed ci.yml <<'YAML'
jobs:
  interface-leakage:
    steps:
      - name: Install ripgrep
        run: |
          sudo apt-get update
          sudo apt-get install -y ripgrep
      - name: Run interface leakage check
        run: bash ci/scripts/test-interface-no-backend-leakage.sh
YAML
expect "the job with the install step is accepted" 0 fixed \
    "All 1 workflow job(s)"

# ------------------------------------------------------------- through a chain
# test-all.sh does not mention rg anywhere; it runs test-coverage-map.sh, which
# does. A gate that reads only the script the workflow names reports this job
# clean — which is what scheduled.yml's weekly job has been.
write_workflow indirect scheduled.yml <<'YAML'
jobs:
  weekly:
    steps:
      - name: Run full test suite
        run: bash ci/scripts/test-all.sh
YAML
expect "a tool needed two scripts deep is found" 1 indirect \
    "weekly" "ripgrep"

# ...and the chain must not be walked into a loop. test-all.sh runs
# test-coverage-map.sh, whose own text names test-all.sh back. Without the
# visited set this recurses until Python gives up, and a crash is not a verdict.
write_workflow cycle ci.yml <<'YAML'
jobs:
  coverage-map:
    steps:
      - run: sudo apt-get install -y ripgrep
      - run: bash ci/scripts/test-coverage-map.sh
YAML
expect "a script that names its own caller terminates" 0 cycle \
    "All 1 workflow job(s)"

# -------------------------------------------------------------- direct use too
# A `run:` block using rg itself, with no script involved.
write_workflow inline ci.yml <<'YAML'
jobs:
  grep-job:
    steps:
      - run: rg -n TODO crates
YAML
expect "rg used directly in a run block counts" 1 inline \
    "grep-job" "ripgrep"

# -------------------------------------------------------- no inventing needs
# The letters are not the tool. `target/rg-out` is a path, `cargo-rg` is a
# hypothetical binary, `codeberg.org/rg` is a URL — a gate that reports any of
# them makes a correct workflow red, which is the failure that gets a gate
# deleted rather than fixed.
write_workflow lookalikes ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          echo target/rg-out
          echo cargo-rg
          echo https://codeberg.org/rg
          echo large
YAML
expect "words merely containing the letters are not the tool" 0 lookalikes \
    "All 1 workflow job(s)" "!ripgrep is"

# A job that genuinely needs nothing stays quiet.
write_workflow clean ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: cargo test --workspace
YAML
expect "a job needing no tools passes" 0 clean \
    "All 1 workflow job(s)" "!::error::"

# ------------------------------------------------------------- job by job
# The install and the use may be different steps of the same job — that is the
# normal shape, one "Install system dependencies" step near the top.
write_workflow across-steps ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: sudo apt-get install -y cmake ripgrep
      - run: cargo build
      - run: bash ci/scripts/test-real-adapter-deps.sh
YAML
expect "an install in an earlier step of the same job counts" 0 across-steps \
    "All 1 workflow job(s)"

# ...but only the SAME job. Jobs run on separate machines, so a sibling job's
# apt line installs nothing here. Scoping this per FILE would call the whole of
# scheduled.yml clean on the strength of one job.
write_workflow other-job ci.yml <<'YAML'
jobs:
  installer:
    steps:
      - run: sudo apt-get install -y ripgrep
  user:
    steps:
      - run: bash ci/scripts/test-real-adapter-deps.sh
YAML
expect "another job's install does not count" 1 other-job \
    "'user'" "ripgrep" "!'installer'"

# A package list continued across a `\` is one command, and two jobs in this
# repository already write theirs that way. Reading the lines separately sees
# only `cmake` and reports a job that installs ripgrep as one that does not —
# a false red, on correct code, from the gate meant to protect it.
write_workflow continuation ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          sudo apt-get install -y \
            cmake g++ \
            ripgrep
      - run: bash ci/scripts/test-real-adapter-deps.sh
YAML
expect "a package list continued across lines is one list" 0 continuation \
    "All 1 workflow job(s)"

# An option is not a package: `-y` must not satisfy anything, and neither must
# the absence of a name after it.
write_workflow options-only ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: sudo apt-get install -y
      - run: rg -n TODO crates
YAML
expect "options alone install nothing" 1 options-only \
    "build" "ripgrep"

# ------------------------------------------------------ what is not a command
# Codex, on a5517fa: a COMMENTED-OUT install credited the job with the package,
# so the gate passed a job that had no ripgrep. This is the dangerous direction
# — a false negative in the gate written to stop false negatives.
write_workflow commented-install ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          # sudo apt-get install -y ripgrep
          echo skipped
      - run: bash ci/scripts/test-real-adapter-deps.sh
YAML
expect "a commented-out install installs nothing" 1 commented-install \
    "build" "ripgrep"

# The same fault with the text merely being printed rather than run.
write_workflow echoed-install ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          echo "run sudo apt-get install -y ripgrep first"
      - run: bash ci/scripts/test-real-adapter-deps.sh
YAML
expect "an install inside an echo installs nothing" 1 echoed-install \
    "build" "ripgrep"

# ...and the mirror image, which the same defect produces in the other
# direction: a commented-out USE is not a use, and reporting it makes a correct
# job red.
write_workflow commented-use ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          # rg -n TODO crates
          cargo build
YAML
expect "a commented-out use needs nothing" 0 commented-use \
    "All 1 workflow job(s)" "!::error::"

# --------------------------------------------------- a use is an INVOCATION
# Codex, on d6df5fd: the tool scan read raw text, so text merely MENTIONING the
# tool demanded the package — a false red on a workflow that never runs it.
write_workflow echoed-use ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo "use rg to search the crates"
YAML
expect "a tool named inside an echo is not a use" 0 echoed-use \
    "All 1 workflow job(s)" "!::error::"

# A heredoc body is data the shell runs none of — and this suite is itself full
# of them, so the gate was demanding ripgrep on account of its own fixtures.
write_workflow heredoc-use ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<'NOTE'
          rg -n TODO crates
          bash ci/scripts/test-real-adapter-deps.sh
          NOTE
YAML
expect "a heredoc body is data, not commands" 0 heredoc-use \
    "All 1 workflow job(s)" "!::error::"

# ...while the real invocations must survive the narrowing. A pipeline element
# is a command, and reading only the first word of the segment would miss it —
# which is exactly how test-real-adapter-deps.sh calls rg.
write_workflow piped-use ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: printf '%s\n' "$tree" | rg -n '^pyo3'
YAML
expect "a tool run as a pipeline element is a use" 1 piped-use \
    "build" "ripgrep"

# An invocation prefix comes before a command without being one.
write_workflow prefixed-use ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: sudo rg -n TODO crates
YAML
expect "a tool behind sudo is a use" 1 prefixed-use \
    "build" "ripgrep"

# A quoted command name still names the command: quotes take a reserved word's
# meaning away and leave a command as it was.
write_workflow quoted-use ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          "rg" -n TODO crates
YAML
expect "a quoted command name is still the command" 1 quoted-use \
    "build" "ripgrep"

# --------------------------------------------- and the forms that hide a use
# Codex, on 03da267, all three verified against the gate before the fix. Each
# is a job that RUNS the tool and was reported clean — the false-negative
# direction, which is the one this gate exists to close.
#
# A script handed to a shell: `command_word` is `bash`, and the program is a
# quoted word nothing looked inside.
write_workflow shell_c ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash -c 'rg -n TODO crates'
YAML
expect "a script handed to bash -c is read" 1 shell_c \
    "build" "ripgrep"

# The same for a repository script run through one.
write_workflow shell_c_script ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash -c 'bash ci/scripts/test-real-adapter-deps.sh'
YAML
expect "a script run from inside bash -c is followed" 1 shell_c_script \
    "build" "ripgrep"

# An executable script run by its own relative path. `./` is how a shell is
# told the path is a path, and the reference pattern rejected it.
write_workflow dot_slash ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: ./ci/scripts/test-real-adapter-deps.sh
YAML
expect "a ./ script path is followed" 1 dot_slash \
    "build" "ripgrep"

write_workflow dot_slash_sourced ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: source ./ci/scripts/test-interface-no-backend-leakage.sh
YAML
expect "a sourced ./ script path is followed" 1 dot_slash_sourced \
    "build" "ripgrep"

# A tool invoked by an absolute path. The command analysis strips directories
# — `/usr/bin/apt-get` is apt-get to the scanner — but the cheap filter in
# front of it excluded `/` as a left boundary and skipped the block entirely.
write_workflow absolute_tool ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: /usr/bin/rg -n TODO crates
YAML
expect "a tool run by absolute path is a use" 1 absolute_tool \
    "build" "ripgrep"

# ------------------------------------------- the forms probing turned up
# Codex named three; probing every invocation form bash has for the same class
# found three more, all reported clean before this commit. A gate that reads
# only the shapes a reviewer happened to mention is a gate that stops at the
# reviewer's imagination.
write_workflow ticked ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: hits=`rg -n TODO crates`
YAML
expect "a backtick substitution runs its contents" 1 ticked \
    "build" "ripgrep"

# shlex gives backticks no meaning, so the opening one arrives welded to the
# name in front of it — `hits=`rg` is one word, and nothing looks for a command
# inside a word. Quoted, the whole substitution is one token instead.
write_workflow ticked-quoted ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: hits="`rg -n TODO crates`"
YAML
expect "a quoted backtick substitution runs its contents" 1 ticked-quoted \
    "build" "ripgrep"

write_workflow substituted ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: hits="$(rg -n TODO crates)"
YAML
expect "a quoted \$() substitution runs its contents" 1 substituted \
    "build" "ripgrep"

# A command whose ARGUMENT is the command that runs.
write_workflow wrapped ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs rg -n TODO
YAML
expect "xargs runs its argument" 1 wrapped \
    "build" "ripgrep"

# ...and past an option's value, which is why a bare number is stepped over.
write_workflow wrapped-numeric ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: timeout 30 rg -n TODO crates
YAML
expect "timeout runs its argument past the duration" 1 wrapped-numeric \
    "build" "ripgrep"

# A function declared and called in the same block runs its body here.
write_workflow function-body ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          search() { rg -n TODO crates; }
          search
YAML
expect "a function body is shell that runs" 1 function-body \
    "build" "ripgrep"

# Both brackets: `f() ( … )` is a function whose body is a subshell.
write_workflow function-subshell ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          search() ( rg -n TODO crates )
          search
YAML
expect "a subshell function body is read too" 1 function-subshell \
    "build" "ripgrep"

# --------------------------------------------------- round four, both ways
# Codex, on f9efff8. Single quotes make a substitution literal text: bash runs
# nothing inside `echo '$(rg …)'`, and the lexer already carries that as
# `literal_dollar` — which this gate was discarding before recursing.
write_workflow literal-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo '$(rg -n TODO crates)'
YAML
expect "a single-quoted substitution runs nothing" 0 literal-substitution \
    "All 1 workflow job(s)" "!::error::"

write_workflow literal-backticks ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo '`rg -n TODO crates`'
YAML
expect "single-quoted backticks run nothing" 0 literal-backticks \
    "All 1 workflow job(s)" "!::error::"

# Process substitution runs its contents in a process of its own, and the
# command word in front of it is something else entirely.
write_workflow process-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: mapfile -t hits < <(rg -n TODO crates)
YAML
expect "a process substitution runs its contents" 1 process-substitution \
    "build" "ripgrep"

# `timeout 30 nice rg …`: timeout's operand is a COMMAND, and that command is
# another wrapper. Taking one step and stopping reads `nice` and never rg.
write_workflow nested-wrappers ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: timeout 30 nice rg -n TODO crates
YAML
expect "wrappers nest" 1 nested-wrappers \
    "build" "ripgrep"

# `bash driver.sh other.sh` runs driver.sh and hands the second path to it as
# `$1`. Following every path that looks like a script rejects a driver for what
# its ARGUMENT does.
write_workflow script-operand ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash ci/scripts/driver.sh ci/scripts/test-real-adapter-deps.sh
YAML
expect "only the shell's own script operand is followed" 0 script-operand \
    "All 1 workflow job(s)" "!::error::"

# The install may be somewhere only the workflow's own context knows. Handing
# the scanner a job stripped of everything but its `run:` strings leaves it
# unable to resolve the reference, and the job is failed for an install it
# makes.
write_workflow job-context ci.yml <<'YAML'
env:
  INSTALL: sudo apt-get install -y ripgrep
jobs:
  build:
    steps:
      - run: ${{ env.INSTALL }}
      - run: rg -n TODO crates
YAML
expect "an install reached through workflow context counts" 0 job-context \
    "All 1 workflow job(s)" "!::error::"

# `find … -exec` names a command in an option rather than at the front.
write_workflow find-exec ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: find crates -exec rg -n TODO {} +
YAML
expect "find -exec runs what follows it" 1 find-exec \
    "build" "ripgrep"

# The stated limitation, pinned so it is a decision and not a surprise: a
# command carried by a variable is not followed. The scanner tracks assignments
# through conditionals, subshells, functions and namerefs to answer this for
# apt, and a weaker copy of that here would be a second answer to the same
# question. The backstop is that the script itself refuses to run without its
# tool, so a job reaching one this way fails loudly rather than silently.
#
# If this case ever goes red, the limitation has been fixed and the comment
# above it is what needs deleting.
write_workflow command-in-a-variable ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          SEARCH="rg -n TODO crates"
          $SEARCH
YAML
expect "a command carried by a variable is not followed" 0 command-in-a-variable \
    "All 1 workflow job(s)"

# ------------------------------------------- round five, my own last commit
# Codex, on 5fd4cef. Every one of these is a consequence of the commit before
# it: a wrapper that resolves to a shell, an option given find's meaning
# everywhere, and a rewrite that dropped the quoting it had just learned to
# respect.
write_workflow wrapped-shell ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: timeout 30 bash -c 'rg -n TODO crates'
YAML
expect "a shell reached through a wrapper is still a shell" 1 wrapped-shell \
    "build" "ripgrep"

write_workflow wrapped-shell-script ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: xargs bash ci/scripts/test-real-adapter-deps.sh
YAML
expect "a script reached through a wrapper is followed" 1 wrapped-shell-script \
    "build" "ripgrep"

# `-exec` is find's word. To `echo` it is an argument like any other.
write_workflow exec-elsewhere ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo -exec rg
YAML
expect "-exec means nothing to a command that is not find" 0 exec-elsewhere \
    "All 1 workflow job(s)" "!::error::"

# Quoted, a process substitution is text: bash performs none inside quotes of
# either kind. The rewrite to `$( … )` was undoing the quoting check that the
# same commit added for `$( … )` itself.
write_workflow quoted-process-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo '<(rg -n TODO crates)'
YAML
expect "a quoted process substitution runs nothing" 0 quoted-process-substitution \
    "All 1 workflow job(s)" "!::error::"

write_workflow double-quoted-process-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo "<(rg -n TODO crates)"
YAML
expect "double quotes stop one too" 0 double-quoted-process-substitution \
    "All 1 workflow job(s)" "!::error::"

# The quoting rule above must stop at PROCESS substitution. Bash still expands
# `$( … )` and still runs backticks inside DOUBLE quotes, so a fix that reads
# "quoted means inert" for every construct would take these two with it.
write_workflow double-quoted-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: hits="$(rg -n TODO crates)"
YAML
expect "double quotes do not stop \$( )" 1 double-quoted-substitution \
    "build" "ripgrep"

write_workflow double-quoted-ticks ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: hits="`rg -n TODO crates`"
YAML
expect "double quotes do not stop backticks" 1 double-quoted-ticks \
    "build" "ripgrep"

# Composites, because each fix in this file was found by a form built out of
# two rules that were each correct on their own.
write_workflow wrapper-wrapper-shell ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: xargs timeout 30 bash -c 'rg -n TODO crates'
YAML
expect "a wrapper behind a wrapper behind a shell" 1 wrapper-wrapper-shell \
    "build" "ripgrep"

write_workflow find-exec-shell ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: find crates -exec bash -c 'rg -n TODO' {} \;
YAML
expect "find -exec reaching a shell program" 1 find-exec-shell \
    "build" "ripgrep"

# ------------------------------------------------------- shapes that are not steps
# A job that calls a reusable workflow has no `steps:` at all. Reading `.steps`
# unguarded makes the gate crash on a real workflow.
write_workflow reusable ci.yml <<'YAML'
jobs:
  call:
    uses: ./.github/workflows/other.yml
YAML
expect "a reusable-workflow job does not crash the gate" 0 reusable \
    "All 1 workflow job(s)"

# A step with `uses:` and no `run:` is most of a real workflow.
write_workflow uses-steps ci.yml <<'YAML'
jobs:
  build:
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
YAML
expect "steps without a run block are skipped" 0 uses-steps \
    "All 1 workflow job(s)"

# The apt scanner is now load-bearing — the gate reads a job's installs through
# it. A copy of the gate with no scanner beside it must refuse to run rather
# than report every job as installing nothing, which is what an empty record
# stream would look like.
cp "$CHECK" "$WORK/lonely-gate.sh"
mkdir -p "$WORK/jobless-dir"
out="$("$WORK/lonely-gate.sh" "$WORK/real/ci.yml" 2>&1)"
got=$?
if [ "$got" -eq 0 ]; then
    fail "the gate ran without the apt scanner beside it"
elif ! printf '%s' "$out" | grep -qF "workflow_apt_deps.py is missing"; then
    fail "the gate failed without the scanner but not for that reason: $out"
else
    ok "the gate refuses to run without the apt scanner"
fi

# ...and a scanner that is THERE but broken is the same hole one step further
# in. It is now both imported (for its lexer) and run (for its records), so
# this fixture kills it at import; either way an empty answer must not read as
# a repository that installs nothing and uses nothing.
mkdir -p "$WORK/brokenscanner"
cp "$CHECK" "$WORK/brokenscanner/gate.sh"
cat >"$WORK/brokenscanner/workflow_apt_deps.py" <<'BROKEN'
import sys

print("scanner exploded", file=sys.stderr)
sys.exit(2)
BROKEN
out="$("$WORK/brokenscanner/gate.sh" "$WORK/inline/ci.yml" 2>&1)"
got=$?
if [ "$got" -eq 0 ]; then
    fail "the gate reported a verdict on records the scanner never produced"
elif ! printf '%s' "$out" | grep -qF "the apt scanner failed"; then
    fail "the gate failed on a broken scanner but not for that reason: $out"
else
    ok "a scanner that fails is not a job that installs nothing"
fi

# ------------------------------------------------------------ it must have run
# A file with no jobs is not a clean file; it is a file the gate could not read
# anything out of, and reporting it as a pass is how an empty scan comes to look
# like a unanimous verdict.
write_workflow jobless ci.yml <<'YAML'
name: nothing
on: push
YAML
expect "a workflow with no jobs is not a pass" 1 jobless \
    "found nothing to check"

expect "a path that does not exist is an error" 1 "no-such-dir/ci.yml" \
    "does not exist"

# A file that is not YAML is not a file with no jobs in it. Ending in a
# traceback says the same thing far less usefully, and this fixture was written
# by accident: a `run: "rg" -n TODO` that YAML rejects.
write_workflow not-yaml ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: "rg" -n TODO crates
YAML
expect "a file that is not YAML says so" 1 not-yaml \
    "is not valid YAML"

# A script NAMED by a command that does not run one is not a script run. It is
# an argument to `echo` here; in test-coverage-map.sh it is
# `coverage_gate="ci/scripts/test-all.sh"`, held so the suite can be checked
# for listing its scripts. Following those pulled in the whole suite — a
# 6,000-line self-test among them, minutes of lexing — and would demand
# ripgrep of any job that so much as printed a path.
#
# The mutation that made this case worth rewriting: taking references from
# every word of every command left the ASSIGNMENT form green either way, since
# an assignment has no command word to reach the rule at all.
write_workflow named-not-run ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo ci/scripts/test-coverage-map.sh
YAML
expect "a script named but not run is not followed" 0 named-not-run \
    "All 1 workflow job(s)" "!::error::"

mkdir -p "$WORK/empty"
expect "a directory with no workflows is an error" 1 empty \
    "no workflow files"

# A script path that does not resolve is passed over in silence, and this case
# exists because the first version announced it: the pattern matches script
# paths inside heredocs, so THIS suite's own fixtures made the gate print a
# notice on every run of the real repository. A missing script is a fault the
# job itself reports at `bash:` on its first run.
write_workflow missing-script ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: sudo apt-get install -y ripgrep
      - run: bash ci/scripts/test-not-a-real-script.sh
YAML
expect "an unresolvable script reference is quiet" 0 missing-script \
    "All 1 workflow job(s)" "!notice" "!::error::"

# --------------------------------------------------- the scripts must say so
# The other half of the fix, and the more important half: the gate above stops
# a job from FORGETTING ripgrep, but nothing stops a script from passing
# silently when it is absent — which is what these three did. Each must now
# fail, and say why, when rg is not on PATH.
#
# The guard has to come before anything else in the script, so an empty PATH is
# enough to reach it; if one of these ever exits for another reason the case
# fails on the message rather than reporting a pass it did not earn.
mkdir -p "$WORK/nobin"
for script in test-interface-no-backend-leakage test-real-adapter-deps test-coverage-map; do
    out="$(env PATH="$WORK/nobin" /bin/bash "$SCRIPT_DIR/$script.sh" 2>&1)"
    got=$?
    if [ "$got" -eq 0 ]; then
        fail "$script passes with no rg on PATH — it checks nothing and says nothing"
    elif ! printf '%s' "$out" | grep -qF "ripgrep (rg) is required"; then
        fail "$script fails without rg but not for that reason: $out"
    else
        ok "$script refuses to run without rg"
    fi
done

# ------------------------------------------------------------------------ done
printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
[ "$FAILED" -eq 0 ]
