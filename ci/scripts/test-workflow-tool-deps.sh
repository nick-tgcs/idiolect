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
    "All 1 workflow job(s)" "!::error::" "!notice"

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

# ------------------------------------------------ round six: find, and a word
# Codex, on a6bf836. Quotes are the SHELL's, and find never sees them.
write_workflow quoted-exec ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: find crates '-exec' rg -n TODO '{}' +
YAML
expect "quoting does not hide find's own action" 1 quoted-exec \
    "build" "ripgrep"

# An action is a COMMAND, so everything known about commands applies inside it.
write_workflow exec-wrapper ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: find crates -exec timeout 30 rg -n TODO {} +
YAML
expect "an action's own wrapper resolves" 1 exec-wrapper \
    "build" "ripgrep"

# `-and` is implicit between adjacent expressions, so BOTH actions run. Reading
# the first and stopping reports a job clean on the strength of its `echo`.
write_workflow two-actions ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: find crates -exec echo {} \; -exec rg -n TODO {} +
YAML
expect "every action in a find expression is read" 1 two-actions \
    "build" "ripgrep"

# A word can be part literal and part live: `'$literal'"$(rg …)"` runs the
# second half. The lexer's flag describes the WORD, so it cannot say which
# dollar is which — and a gate that cannot tell says so rather than deciding.
write_workflow mixed-quoting ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo '$literal'"$(rg -n TODO crates)"
YAML
expect "a part-literal word is announced, not decided" 0 mixed-quoting \
    "notice:" "cannot tell"

# The action that matters may be the FIRST one as easily as the second, and
# `-execdir` is the same action from another directory.
write_workflow two-actions-first ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: find crates -exec rg -n TODO {} \; -exec echo {} +
YAML
expect "an action before another is read too" 1 two-actions-first \
    "build" "ripgrep"

write_workflow execdir ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: find crates -execdir rg -n TODO {} +
YAML
expect "-execdir is the same action" 1 execdir \
    "build" "ripgrep"

# find itself reached through a wrapper, with the tool inside its action: three
# rules composed, which is how every hole in this file has been found.
write_workflow wrapped-find ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: xargs find crates -exec rg -n TODO {} +
YAML
expect "a wrapper, a find, and an action" 1 wrapped-find \
    "build" "ripgrep"

# find with no action at all names no command.
write_workflow find-plain ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: find crates -name "*.rs"
YAML
expect "a find with no action runs nothing" 0 find-plain \
    "All 1 workflow job(s)" "!::error::"

# Codex, on dcf9333: quoting a word INSIDE a process substitution is not
# quoting the substitution. Dropping every quoted word before the rewrite threw
# away the command name and left `<( … )` running nothing.
write_workflow quoted-inside-process-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: mapfile -t hits < <("rg" -n TODO crates)
YAML
expect "a quoted command inside a process substitution runs" 1 quoted-inside-process-substitution \
    "build" "ripgrep"

write_workflow single-quoted-inside-process-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: mapfile -t hits < <('rg' -n TODO crates)
YAML
expect "single quotes inside one do not stop it either" 1 single-quoted-inside-process-substitution \
    "build" "ripgrep"

# Found by probing the fix above rather than by review: an unquoted `$( … )`
# in ARGUMENT position. shlex splits it into `$` and `(`, so no word holds it,
# and `command_word` stops at `echo` long before reaching the tool. It was seen
# in `hits=$(rg …)` only because an assignment has no command word to stop at.
write_workflow substitution-as-argument ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo $(rg -n TODO crates)
YAML
expect "an unquoted substitution in argument position runs" 1 substitution-as-argument \
    "build" "ripgrep"

write_workflow substitution-inside-process-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: mapfile -t x < <(echo $(rg -n TODO crates))
YAML
expect "a substitution inside a process substitution runs" 1 substitution-inside-process-substitution \
    "build" "ripgrep"

write_workflow nested-substitutions ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo $(echo $(rg -n TODO crates))
YAML
expect "substitutions nest" 1 nested-substitutions \
    "build" "ripgrep"

write_workflow ticks-as-argument ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo `rg -n TODO crates`
YAML
expect "backticks in argument position run" 1 ticks-as-argument \
    "build" "ripgrep"

# Codex, on e9866ba: a QUOTED parenthesis opens nothing. `<"("` redirects from
# a file named `(`, and rejoining the words without their quoting produced a
# `<(` that was never written. The word carrying the opener decides, which is
# the same rule as the two above it — only now the opener is a word of its own.
write_workflow quoted-opener ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo hi <"(" rg -n TODO crates ")"
YAML
expect "a quoted parenthesis opens no substitution" 0 quoted-opener \
    "All 1 workflow job(s)" "!::error::"

# Codex, on d3039b5: a heredoc body is data, but an UNQUOTED delimiter means
# bash expands that data first — so a substitution written in one runs. The
# delimiter decides, and it is written on the opening line.
write_workflow expanding-heredoc ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<EOF
          found: $(rg -n TODO crates)
          EOF
YAML
expect "a substitution in an expanding heredoc runs" 1 expanding-heredoc \
    "build" "ripgrep"

write_workflow expanding-heredoc-ticks ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<EOF
          found: `rg -n TODO crates`
          EOF
YAML
expect "backticks in an expanding heredoc run" 1 expanding-heredoc-ticks \
    "build" "ripgrep"

# Quoted, nothing in the body is expanded and the same text is inert. Both
# spellings of quoting the delimiter, since either one stops it.
write_workflow quoted-heredoc-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<'EOF'
          found: $(rg -n TODO crates)
          EOF
YAML
expect "a quoted delimiter leaves the body literal" 0 quoted-heredoc-substitution \
    "All 1 workflow job(s)" "!::error::"

write_workflow double-quoted-heredoc ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<"EOF"
          found: $(rg -n TODO crates)
          EOF
YAML
expect "double-quoting the delimiter stops it too" 0 double-quoted-heredoc \
    "All 1 workflow job(s)" "!::error::"

# Codex, on 4e02d11: one command may open SEVERAL heredocs, and each
# delimiter answers the question for its own body. Attributing a body to its
# own opener needs the terminator lines, which the scanner consumes — so where
# a command's delimiters disagree, neither body is read.
#
# Announcing it instead was tried and withdrawn: the notice fired on this
# repository's own workflows, because THIS file — fixtures quoting both
# spellings — lexes as a single block, and the notice printed all of it. A
# limitation whose direction is a use unseen, in a construct no workflow here
# writes, with the script's own guard still behind it.
write_workflow mixed-heredocs ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<'LITERAL' <<EXPANDING
          one $(rg -n one crates)
          LITERAL
          two $(rg -n two crates)
          EXPANDING
YAML
expect "disagreeing delimiters read neither body" 0 mixed-heredocs \
    "All 1 workflow job(s)" "!::error::" "!notice"

# Agreeing ones are still decided, in both directions.
write_workflow two-expanding-heredocs ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<A <<B
          one $(rg -n one crates)
          A
          two
          B
YAML
expect "two expanding heredocs are read" 1 two-expanding-heredocs \
    "build" "ripgrep"

write_workflow two-literal-heredocs ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<'A' <<'B'
          one $(rg -n one crates)
          A
          two
          B
YAML
expect "two literal heredocs stay literal" 0 two-literal-heredocs \
    "All 1 workflow job(s)" "!::error::"

# Codex, on 4e02d11: a wrapper's own operands are not its command. `-I` takes
# a replacement string, and timeout takes a DURATION which may carry a suffix,
# so neither `{}` nor `30s` is the thing being run.
write_workflow xargs-replace ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs -I '{}' rg -n TODO '{}'
YAML
expect "an option's own argument is not the command" 1 xargs-replace \
    "build" "ripgrep"

write_workflow timeout-suffix ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: timeout 30s rg -n TODO crates
YAML
expect "a duration with a suffix is not the command" 1 timeout-suffix \
    "build" "ripgrep"

write_workflow timeout-signal ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: timeout -k 5s 30s rg -n TODO crates
YAML
expect "an option, its argument, and then the duration" 1 timeout-signal \
    "build" "ripgrep"

write_workflow nice-adjustment ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: nice -n 5 rg -n TODO crates
YAML
expect "nice takes an adjustment before its command" 1 nice-adjustment \
    "build" "ripgrep"

# ------------------------------------------------ round eleven, all missed uses
# Codex, on 951f419. `--replace[=R]` and `--eof[=END]` take an ATTACHED value,
# so a bare one consumes nothing — listing them as options with a separate
# argument ate the command itself.
write_workflow xargs-optional-value ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs --replace rg -n TODO
YAML
expect "an optional attached value consumes nothing" 1 xargs-optional-value \
    "build" "ripgrep"

# Quoting an option does not stop it being one: the quotes are the shell's and
# xargs is handed `-I` either way. Same lesson as find's own `-exec`.
write_workflow quoted-wrapper-option ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs "-I" "{}" rg -n TODO "{}"
YAML
expect "a quoted wrapper option is still an option" 1 quoted-wrapper-option \
    "build" "ripgrep"

# A wrapper may resolve onto an invocation PREFIX, which the command reader
# handles at the front of a command and nothing re-applied after the walk.
write_workflow wrapper-then-prefix ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: timeout 30 env rg -n TODO crates
YAML
expect "a prefix reached through a wrapper is still a prefix" 1 wrapper-then-prefix \
    "build" "ripgrep"

write_workflow wrapper-then-sudo ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs sudo rg -n TODO
YAML
expect "sudo behind a wrapper is stepped over too" 1 wrapper-then-sudo \
    "build" "ripgrep"

# A heredoc delimiter is a WORD, not an identifier: `<<123` is legal and
# unquoted, so its body expands.
write_workflow numeric-delimiter ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<123
          found: $(rg -n TODO crates)
          123
YAML
expect "a numeric heredoc delimiter is a delimiter" 1 numeric-delimiter \
    "build" "ripgrep"

# ...and quoting one still stops the expansion.
write_workflow quoted-numeric-delimiter ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<'123'
          found: $(rg -n TODO crates)
          123
YAML
expect "a quoted numeric delimiter stays literal" 0 quoted-numeric-delimiter \
    "All 1 workflow job(s)" "!::error::"

# The spelling GitHub's own documentation uses for a repository path.
write_workflow workspace-prefixed ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash ${{ github.workspace }}/ci/scripts/test-real-adapter-deps.sh
YAML
expect "a workspace-prefixed script path is followed" 1 workspace-prefixed \
    "build" "ripgrep"

write_workflow absolute-script-path ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash /home/runner/work/idiolect/idiolect/ci/scripts/test-real-adapter-deps.sh
YAML
expect "an absolute script path is followed" 1 absolute-script-path \
    "build" "ripgrep"

# ...but a path that merely ENDS in those characters is a different file.
write_workflow lookalike-path ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash vendor/notci/scripts/test-real-adapter-deps.sh
YAML
expect "a path ending in the same letters is not the script" 0 lookalike-path \
    "All 1 workflow job(s)" "!::error::"

# ------------------------------------------------------------ round twelve
# Codex, on d32dd1d. A heredoc body arrives a line at a time, so a
# substitution written across lines was never whole in any of them.
write_workflow multiline-heredoc-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<EOF
          found: $(
            rg -n TODO crates
          )
          EOF
YAML
expect "a substitution spanning heredoc lines runs" 1 multiline-heredoc-substitution \
    "build" "ripgrep"

# Escaped backticks nest a legacy substitution inside another.
write_workflow escaped-backticks ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo `echo \`rg -n TODO crates\``
YAML
expect "escaped backticks nest a substitution" 1 escaped-backticks \
    "build" "ripgrep"

# Quoting ANY part of a delimiter disables expansion in its body, and a
# backslash is quoting too.
write_workflow backslash-delimiter ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<\EOF
          found: $(rg -n TODO crates)
          EOF
YAML
expect "a backslash-quoted delimiter stays literal" 0 backslash-delimiter \
    "All 1 workflow job(s)" "!::error::"

write_workflow part-quoted-delimiter ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<E"OF"
          found: $(rg -n TODO crates)
          EOF
YAML
expect "a part-quoted delimiter stays literal" 0 part-quoted-delimiter \
    "All 1 workflow job(s)" "!::error::"

# A single quote inside DOUBLE quotes is a literal character, not a quote, so
# the backticks between them still run. Written down because I expected the
# opposite while probing and the gate was right.
write_workflow apostrophe-in-double ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo "outer '`rg -n TODO crates`'"
YAML
expect "an apostrophe inside double quotes stops nothing" 1 apostrophe-in-double \
    "build" "ripgrep"

# Codex, on 06f621c: inside an expanding heredoc a backslash still quotes, so
# `\$(rg …)` is written out rather than run. The same goes for a backtick.
write_workflow escaped-dollar-heredoc ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<EOF
          literally: \$(rg -n TODO crates)
          EOF
YAML
expect "an escaped dollar in a heredoc runs nothing" 0 escaped-dollar-heredoc \
    "All 1 workflow job(s)" "!::error::"

write_workflow escaped-tick-heredoc ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<EOF
          literally: \`rg -n TODO crates\`
          EOF
YAML
expect "an escaped backtick in a heredoc runs nothing" 0 escaped-tick-heredoc \
    "All 1 workflow job(s)" "!::error::"

# ...and the unescaped one beside it still runs, so the blanking cannot simply
# swallow the body.
write_workflow escaped-and-live-heredoc ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<EOF
          literally: \$(echo nothing)
          really: $(rg -n TODO crates)
          EOF
YAML
expect "a live substitution beside an escaped one still runs" 1 escaped-and-live-heredoc \
    "build" "ripgrep"

# The escaped pair is blanked rather than deleted, and this is the case that
# says why: delete it from `$\$(rg …)` and the two halves close up into a
# `$(` nobody wrote. Bash runs nothing here — a `$` before a backslash is a
# dollar, and the escaped one after it is another.
write_workflow escape-closing-up ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<EOF
          literally: $\$(rg -n TODO crates)
          EOF
YAML
expect "blanking an escape cannot create a substitution" 0 escape-closing-up \
    "All 1 workflow job(s)" "!::error::"

# Codex, on 5fece60: bash's short options CLUSTER, and `-o` takes its argument
# from the next word even at the end of one — so `pipefail` was read as the
# script and the real one never followed.
write_workflow clustered-options ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash -euo pipefail ci/scripts/test-real-adapter-deps.sh
YAML
expect "a clustered -o takes the word after it" 1 clustered-options \
    "build" "ripgrep"

write_workflow plus-options ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash +o posix ci/scripts/test-real-adapter-deps.sh
YAML
expect "+o takes one too" 1 plus-options \
    "build" "ripgrep"

# ...and a cluster WITHOUT one must not swallow the script.
write_workflow clustered-no-argument ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash -eu ci/scripts/test-real-adapter-deps.sh
YAML
expect "a cluster with no such option swallows nothing" 1 clustered-no-argument \
    "build" "ripgrep"

# Codex, on 94be871, and confirmed against bash 5.2.21 rather than taken on
# faith: `-o` ANYWHERE in a cluster consumes the word after it, not only at the
# end. `bash -oe pipefail script.sh` runs the script with both errexit and
# pipefail set.
write_workflow cluster-o-first ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash -oe pipefail ci/scripts/test-real-adapter-deps.sh
YAML
expect "-o earlier in a cluster still takes a word" 1 cluster-o-first \
    "build" "ripgrep"

write_workflow cluster-O ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash -Oe nullglob ci/scripts/test-real-adapter-deps.sh
YAML
expect "-O is the same" 1 cluster-O \
    "build" "ripgrep"

# Each of them takes its OWN word: `bash -oO pipefail nullglob script.sh` runs
# the script, which the probe confirmed by watching both settings change.
write_workflow cluster-two-options ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash -oO pipefail nullglob ci/scripts/test-real-adapter-deps.sh
YAML
expect "two option letters take two words" 1 cluster-two-options \
    "build" "ripgrep"

# ----------------------------------------------------------- round sixteen
# Codex, on c1c9438, each verified against the tool itself first.
#
# `bash -oc pipefail 'rg …'` sets pipefail from the next word and runs the one
# after it. The fix is in the apt scanner, whose `script_argument` this gate
# asks — the same defect would have hidden an apt install written that way.
write_workflow cluster-o-and-c ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash -oc pipefail 'rg -n TODO crates'
YAML
expect "an -o operand before a -c script is consumed" 1 cluster-o-and-c \
    "build" "ripgrep"

# `cat <(echo ")"; rg …)` runs rg: the quoted bracket is data. Dropping only
# quoted OPENERS left the quoted closer in the rejoined line, where it closed
# the substitution early and left the tool outside it.
write_workflow quoted-closer ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: cat <(echo ")"; rg -n TODO crates)
YAML
expect "a quoted closing bracket closes nothing" 1 quoted-closer \
    "build" "ripgrep"

# `xargs -rn 1 rg …` — a wrapper's short options cluster too, and `-n` takes
# its value from the next word.
write_workflow wrapper-cluster ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs -rn 1 rg -n TODO
YAML
expect "a clustered wrapper option takes its value" 1 wrapper-cluster \
    "build" "ripgrep"

# ...and one whose letters take nothing must not eat the command.
write_workflow wrapper-cluster-plain ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs -rt rg -n TODO
YAML
expect "a cluster of valueless options eats nothing" 1 wrapper-cluster-plain \
    "build" "ripgrep"

# Codex, on 1c9cf3e: a short option's value may be ATTACHED, and then it takes
# no word at all. Confirmed against the tools themselves — `xargs -n1`,
# `xargs -rn1`, `xargs -I{}`, `timeout -k5` and `nice -n5` all work, and each
# runs the command that follows.
write_workflow attached-value ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs -n1 rg -n TODO
YAML
expect "an attached value takes no word" 1 attached-value \
    "build" "ripgrep"

write_workflow attached-in-cluster ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs -rn1 rg -n TODO
YAML
expect "attached inside a cluster too" 1 attached-in-cluster \
    "build" "ripgrep"

write_workflow attached-replacement ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs -I{} rg -n TODO {}
YAML
expect "a replacement string may be attached" 1 attached-replacement \
    "build" "ripgrep"

write_workflow attached-kill-after ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: timeout -k5 30 rg -n TODO crates
YAML
expect "an attached option value before a duration" 1 attached-kill-after \
    "build" "ripgrep"

write_workflow attached-nice ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: nice -n5 rg -n TODO crates
YAML
expect "nice takes its adjustment attached" 1 attached-nice \
    "build" "ripgrep"

# Codex, on 6a4cc47: a job's needs are followed through the scripts it runs and
# its INSTALLS are not, so a script that installs its own tool is reported as a
# job that forgot it. The asymmetry is real and this pins it as a LIMITATION
# rather than a fix, because the fix was measured and costs more than it buys:
# handing a script to the apt scanner takes over thirty seconds on this
# repository's own self-test, and pulling out the install-looking LINES instead
# loses the context that makes them right — a fixture install inside a heredoc
# would be credited as a real one, which is a false green.
#
# The direction here is a false RED, fixed by naming the package in the
# workflow like the other 38 jobs do. If this case ever fails, the limitation
# has been fixed and the comment in the gate needs deleting with it.
mkdir -p "$WORK/fixture-repo/ci/scripts"
cat >"$WORK/fixture-repo/ci/scripts/installer.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
sudo apt-get install -y ripgrep
rg -n TODO crates
SCRIPT
write_workflow script-installs ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash ci/scripts/installer.sh
YAML
out="$(REPO_ROOT="$WORK/fixture-repo" "$CHECK" "$WORK/script-installs/ci.yml" 2>&1)"
got=$?
if [ "$got" -eq 0 ]; then
    fail "the limitation above has been fixed — delete this case and the comment with it"
elif ! printf '%s' "$out" | grep -qF "ripgrep"; then
    fail "the script-install case failed for another reason: $out"
else
    ok "a script's own install is not credited (stated limitation)"
fi

# ...and the other half: a script that uses the tool without installing it is
# still a finding, so the fix cannot simply credit every job.
cat >"$WORK/fixture-repo/ci/scripts/user.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
rg -n TODO crates
SCRIPT
write_workflow script-uses ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash ci/scripts/user.sh
YAML
out="$(REPO_ROOT="$WORK/fixture-repo" "$CHECK" "$WORK/script-uses/ci.yml" 2>&1)"
got=$?
if [ "$got" -eq 0 ]; then
    fail "a script using the tool with no install was accepted: $out"
elif ! printf '%s' "$out" | grep -qF "ripgrep"; then
    fail "the script-use case failed for another reason: $out"
else
    ok "a script that only uses the tool is still a finding"
fi

# Quoting a shell's own option does not stop it: `bash "-c" '…'` runs the
# string. Fixed in the apt scanner, since the same reading would hide an
# install written that way.
write_workflow quoted-dash-c ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash "-c" 'rg -n TODO crates'
YAML
expect "a quoted -c still hands over a script" 1 quoted-dash-c \
    "build" "ripgrep"

write_workflow quoted-cluster-c ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash '-ec' 'rg -n TODO crates'
YAML
expect "a quoted cluster hands one over too" 1 quoted-cluster-c \
    "build" "ripgrep"

# Codex, on e08e4a1, confirmed against bash 5.2.21: in an EXPANDING heredoc a
# backslash-newline is removed before expansion, so a `$` at the end of one
# line and a `(` at the start of the next are one substitution and it runs.
# Blanking the pair like any other escape left `$  (rg …)`, which is nothing.
write_workflow heredoc-continuation ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<EOF
          found: $\
          (rg -n TODO crates)
          EOF
YAML
expect "a continuation inside a heredoc joins the line" 1 heredoc-continuation \
    "build" "ripgrep"

# The same body under a quoted delimiter is printed as written — the probe
# showed bash leaving both the backslash and the newline in place.
write_workflow heredoc-continuation-literal ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<'EOF'
          found: $\
          (rg -n TODO crates)
          EOF
YAML
expect "a quoted delimiter keeps the continuation literal" 0 heredoc-continuation-literal \
    "All 1 workflow job(s)" "!::error::"

# Codex, on 432effa: `-l` takes its value ATTACHED or not at all — `-l[MAX]` —
# so a bare one consumes nothing and the word after it is the command. Probed,
# and the same probe drew the line for its neighbours:
#
#   printf 'x\n' | xargs -l echo RAN     RAN x        (bare: nothing taken)
#   printf 'a\nb\n' | xargs -l2 echo RAN  RAN a b      (attached)
#   printf 'x\n' | xargs -L echo RAN     invalid number "echo"  (REQUIRED)
#   printf 'x\n' | xargs -L 1 echo RAN   RAN x
#
# So three classes, not two: required, optional-if-attached, and none.
write_workflow optional-value-bare ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs -l rg -n TODO
YAML
expect "a bare optional-value option takes nothing" 1 optional-value-bare \
    "build" "ripgrep"

write_workflow optional-value-attached ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs -l2 rg -n TODO
YAML
expect "the same option with its value attached" 1 optional-value-attached \
    "build" "ripgrep"

write_workflow required-value-separate ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs -L 1 rg -n TODO
YAML
expect "-L still takes its value from the next word" 1 required-value-separate \
    "build" "ripgrep"

write_workflow optional-replacement ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs -i rg -n TODO {}
YAML
expect "a bare -i takes nothing either" 1 optional-replacement \
    "build" "ripgrep"

# Codex, on 07344b4: a command name can be ASSEMBLED out of quoting — `r\g`,
# `r"g"` and `'r'g` all run rg, each confirmed against a proxy on PATH. The
# lexer normalises them; the cheap filter in front of it was looking at the raw
# line and saw no `rg` at all, so nothing was ever lexed.
#
# The same class as `/usr/bin/rg` five rounds ago: a filter narrower than the
# reader behind it decides the verdict by itself. Fixed generally — quoting
# characters are removed before the filter looks — rather than by widening one
# pattern.
write_workflow escaped-name ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: r\g -n TODO crates
YAML
expect "a backslash inside the command name" 1 escaped-name \
    "build" "ripgrep"

write_workflow quoted-half-name ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: r"g" -n TODO crates
YAML
expect "half the name in quotes" 1 quoted-half-name \
    "build" "ripgrep"

write_workflow other-half-quoted ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          'r'g -n TODO crates
YAML
expect "the other half in quotes" 1 other-half-quoted \
    "build" "ripgrep"

# A script path assembles the same way, and the same filter hid it.
write_workflow assembled-path ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash ci/scripts/"test-real-adapter-deps.sh"
YAML
expect "a script path assembled out of quotes is followed" 1 assembled-path \
    "build" "ripgrep"

# --------------------------------------------- round twenty-two, all probed
# Codex, on 631fa9f. The long spelling of an option takes its value the same
# way the short one does, and `--max-chars` was simply missing.
write_workflow long-option-value ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs --max-chars 100 rg -n TODO
YAML
expect "a long option takes its value too" 1 long-option-value \
    "build" "ripgrep"

# In an expanding heredoc a backslash is special only before `$`, a backtick,
# another backslash, or a newline. Everywhere else it survives into the
# substitution, which does its own quote removal — `$(r\g …)` runs rg, checked
# against a proxy on PATH.
write_workflow heredoc-ordinary-backslash ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<EOF
          out: $(r\g -n TODO crates)
          EOF
YAML
expect "an ordinary backslash survives into a substitution" 1 heredoc-ordinary-backslash \
    "build" "ripgrep"

# An escaped backtick is data, and the LIVE pair after it still runs. Pairing
# the escaped one with the live opener left the real closer unmatched.
write_workflow escaped-then-live-tick ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          echo \`literal `rg -n TODO crates`
YAML
expect "a live backtick pair after an escaped one" 1 escaped-then-live-tick \
    "build" "ripgrep"

# A quoted option that takes a value is still one: `bash "-O" nullglob -c '…'`
# consumes nullglob and runs the string. Fixed in the apt scanner, where the
# same reading would hide an install.
write_workflow quoted-option-with-value ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash "-O" nullglob -c 'rg -n TODO crates'
YAML
expect "a quoted option still takes its value" 1 quoted-option-with-value \
    "build" "ripgrep"

# ------------------------------------------- round twenty-three, all probed
# Codex, on 92bf47b. `xargs --help rg` prints usage and exits — the option
# ENDS the invocation, and reading past it invented a command.
write_workflow terminal-option ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs --help rg
YAML
expect "an option that ends the invocation runs nothing" 0 terminal-option \
    "All 1 workflow job(s)" "!::error::"

# Two backslashes are ONE literal backslash, and the substitution after them is
# still live: `echo \\$(rg …)` prints a backslash and rg's output. The lexer
# says so — it sets `literal_dollar` for `\$` and not for `\\$` — and the
# rejoin was asking about whole-word quoting instead.
write_workflow even-backslashes ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          echo \\$(rg -n TODO crates)
YAML
expect "an even backslash run leaves the substitution live" 1 even-backslashes \
    "build" "ripgrep"

write_workflow even-backslashes-tick ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          echo \\`rg -n TODO crates`
YAML
expect "an even run leaves a backtick opener live too" 1 even-backslashes-tick \
    "build" "ripgrep"

# ...and the odd run still quotes what follows it.
write_workflow odd-backslash ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          echo \$(rg -n TODO crates)
YAML
expect "an odd backslash run still quotes the dollar" 0 odd-backslash \
    "All 1 workflow job(s)" "!::error::"

# ANSI-C quoting: the lexer resolves `$'r'$'g'` and even `$'\x72g'` to rg, and
# the cheap filter in front of it could not. Filters do not decide any more —
# a workflow's own `run:` is always lexed.
write_workflow ansi-c-name ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          $'r'$'g' -n TODO crates
YAML
expect "a name spelled in ANSI-C quoting" 1 ansi-c-name \
    "build" "ripgrep"

write_workflow ansi-c-hex ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          $'\x72g' -n TODO crates
YAML
expect "a name hidden in a hex escape" 1 ansi-c-hex \
    "build" "ripgrep"

# ------------------------------------------ round twenty-four, all probed
# Codex, on e9ab98e. Three of these four are mine: I added `--show-limits` and
# `--usage` to the terminal set last round without running either. The probe
# says `xargs --show-limits rg …` RUNS rg, and `--usage` is not an xargs option
# at all. Nothing goes in these tables now without a probe beside it.
write_workflow show-limits ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs --show-limits rg -n TODO
YAML
expect "--show-limits does not end the invocation" 1 show-limits \
    "build" "ripgrep"

# GNU accepts an unambiguous abbreviation of a long option, and it takes its
# value the same way: `xargs --max-a 1 rg …` runs rg with `1` consumed.
write_workflow abbreviated-option ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs --max-a 1 rg -n TODO
YAML
expect "an abbreviated long option takes its value" 1 abbreviated-option \
    "build" "ripgrep"

# ...and an AMBIGUOUS abbreviation is not resolved to one of them.
write_workflow ambiguous-abbreviation ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: echo crates | xargs --max rg -n TODO
YAML
expect "an ambiguous abbreviation takes nothing" 1 ambiguous-abbreviation \
    "build" "ripgrep"

# util-linux spells help and version short: `ionice -h rg` prints usage and
# `setsid -V rg` prints a version, and neither runs rg.
write_workflow ionice-help ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: ionice -h rg -n TODO crates
YAML
expect "a short help option ends it too" 0 ionice-help \
    "All 1 workflow job(s)" "!::error::"

write_workflow setsid-version ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: setsid -V rg -n TODO crates
YAML
expect "a short version option ends it too" 0 setsid-version \
    "All 1 workflow job(s)" "!::error::"

# A shell has them as well: `bash --help script.sh` prints help and runs no
# script.
write_workflow bash-help ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash --help ci/scripts/test-real-adapter-deps.sh
YAML
expect "a shell's terminal option runs no script" 0 bash-help \
    "All 1 workflow job(s)" "!::error::"

# Codex, on 707d953: `setsid` and `nohup` are wrappers with no entry in the
# long-option map, so their abbreviations resolved to nothing and the command
# after them was read as one. Probed: `setsid --ver rg` prints a version and
# `nohup --he rg` prints usage; neither runs rg.
write_workflow setsid-abbreviated ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: setsid --ver rg -n TODO crates
YAML
expect "an abbreviation of a wrapper's own version option" 0 setsid-abbreviated \
    "All 1 workflow job(s)" "!::error::"

write_workflow nohup-abbreviated ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: nohup --he rg -n TODO crates
YAML
expect "and of its help option" 0 nohup-abbreviated \
    "All 1 workflow job(s)" "!::error::"

# ...and the reason this keeps happening is a table filled in one entry at a
# time. A wrapper with no option list is now refused outright rather than
# quietly resolving nothing, so the next one added cannot be forgotten.
mkdir -p "$WORK/short-table"
cp "$CHECK" "$WORK/short-table/gate.sh"
cp "$SCRIPT_DIR/workflow_apt_deps.py" "$WORK/short-table/"
python3 - "$WORK/short-table/gate.sh" <<'PYTHON'
import sys

path = sys.argv[1]
text = open(path).read()
old = '    "nohup": ("--help", "--version"),\n'
assert text.count(old) == 1, "the map entry this case removes has moved"
open(path, "w").write(text.replace(old, "", 1))
PYTHON
out="$("$WORK/short-table/gate.sh" "$WORK/setsid-abbreviated/ci.yml" 2>&1)"
got=$?
if [ "$got" -eq 0 ]; then
    fail "a wrapper with no option list was accepted in silence"
elif ! printf '%s' "$out" | grep -qF "no option list"; then
    fail "the short-table copy failed for another reason: $out"
else
    ok "a wrapper with no option list is refused"
fi

# Codex, on 4967f33: `coproc rg …` runs rg asynchronously, and reading
# `coproc` as the command missed it. Fixed in the apt scanner's `command_word`,
# where the same gap hid an install. Both forms probed by reading the
# coprocess's pipe — the output does not reach the terminal on its own.
write_workflow coproc-simple ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: coproc rg -n TODO crates
YAML
expect "coproc introduces a command" 1 coproc-simple \
    "build" "ripgrep"

write_workflow coproc-named ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: coproc FOO { rg -n TODO crates; }
YAML
expect "a named coproc runs what is in its braces" 1 coproc-named \
    "build" "ripgrep"

# Codex, on bcf9cab: `time -p rg …` runs rg, and skipping the reserved word
# left `-p` as the command. `help time` gives the syntax as `time [-p]
# pipeline`, and bash takes a `--` too — all three forms probed.
write_workflow time-portable ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: time -p rg -n TODO crates
YAML
expect "time's own option is not the command" 1 time-portable \
    "build" "ripgrep"

write_workflow time-separator ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: time -- rg -n TODO crates
YAML
expect "and neither is its separator" 1 time-separator \
    "build" "ripgrep"

write_workflow time-both ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: time -p -- rg -n TODO crates
YAML
expect "nor both of them together" 1 time-both \
    "build" "ripgrep"

# ...and `-p` is `time`'s alone. After any other reserved word it is a command
# name of its own — `{ -p rg …; }` reports `-p: command not found` and never
# reaches rg, so requiring ripgrep there would be a false red. This case exists
# because the mutation that widened the rule to every reserved word survived
# without it.
write_workflow dash-p-elsewhere ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          { -p rg -n TODO crates; }
YAML
expect "-p after another reserved word is a command" 0 dash-p-elsewhere \
    "All 1 workflow job(s)" "!::error::"

# ------------------------------------------- round twenty-eight, both probed
# Codex, on 34a9c61: a heredoc fed to `bash -s` is the child shell's SCRIPT,
# not data. Probed — the body runs. The apt scanner announces this case as a
# heredoc it cannot check; this gate was silently blind to it, which is the
# false green it exists to prevent.
write_workflow shell-stdin-heredoc ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          bash -s <<'SCRIPT'
          rg -n TODO crates
          SCRIPT
YAML
expect "a heredoc fed to a shell is a script" 1 shell-stdin-heredoc \
    "build" "ripgrep"

write_workflow shell-stdin-unquoted ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          bash <<SCRIPT
          rg -n TODO crates
          SCRIPT
YAML
expect "with no -s and an unquoted delimiter either" 1 shell-stdin-unquoted \
    "build" "ripgrep"

# ...but a heredoc fed to something that is NOT a shell is still data.
write_workflow cat-heredoc-still-data ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          cat <<'SCRIPT'
          rg -n TODO crates
          SCRIPT
YAML
expect "a heredoc fed to cat is still data" 0 cat-heredoc-still-data \
    "All 1 workflow job(s)" "!::error::"

# `$(( … ))` is ARITHMETIC: bash reads `rg` there as a variable and runs no
# command. Reading the outer `$(` as a substitution demanded ripgrep for
# `echo $((rg + 1))`, which prints a number.
write_workflow arithmetic ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          rg=41
          echo $((rg + 1))
YAML
expect "arithmetic is not a command substitution" 0 arithmetic \
    "All 1 workflow job(s)" "!::error::"

# ...and a real substitution INSIDE arithmetic still runs.
write_workflow arithmetic-with-substitution ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          echo $(( $(rg -c TODO crates) + 1 ))
YAML
expect "a substitution inside arithmetic still runs" 1 arithmetic-with-substitution \
    "build" "ripgrep"

# ...and a heredoc fed to a shell that HAS a script operand is data for that
# script, not code. Probed: `bash reads-stdin.sh <<EOF` prints the body as its
# standard input and runs none of it. This case exists because the mutation
# that dropped the operand test survived without it.
write_workflow heredoc-past-a-script ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          bash ci/scripts/test-not-a-real-script.sh <<'EOF'
          rg -n TODO crates
          EOF
YAML
expect "a heredoc past a script operand is data" 0 heredoc-past-a-script \
    "All 1 workflow job(s)" "!::error::"

# Codex, on 0114610, both probed and both consequences of the commit before:
# a here-STRING is a stdin script too, and an option's own argument is not a
# script operand.
write_workflow here-string-script ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash <<< 'rg -n TODO crates'
YAML
expect "a here-string fed to a shell is a script" 1 here-string-script \
    "build" "ripgrep"

write_workflow here-string-with-s ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash -s <<< 'rg -n TODO crates'
YAML
expect "with -s as well" 1 here-string-with-s \
    "build" "ripgrep"

write_workflow option-operand-then-heredoc ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          bash -O nullglob <<'SCRIPT'
          rg -n TODO crates
          SCRIPT
YAML
expect "an option's argument is not a script operand" 1 option-operand-then-heredoc \
    "build" "ripgrep"

# Codex, on df5d0f1: with `-s` the operands are POSITIONAL, not a script file,
# so the heredoc is still the script. Probed — the body runs and the operand
# arrives as `$*`.
write_workflow dash-s-positional ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          bash -s positional <<'SCRIPT'
          rg -n TODO crates
          SCRIPT
YAML
expect "-s makes later words positional, not a script" 1 dash-s-positional \
    "build" "ripgrep"

write_workflow dash-s-clustered ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          bash -es positional <<'SCRIPT'
          rg -n TODO crates
          SCRIPT
YAML
expect "and it may be inside a cluster" 1 dash-s-clustered \
    "build" "ripgrep"

# ...and it has to be `-s` in particular. Any OTHER option leaves the operand
# a script file, so the heredoc is that script's input and runs nothing — this
# case exists because the mutation that accepted any short option survived
# without it.
write_workflow other-option-then-script ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          bash -e ci/scripts/test-not-a-real-script.sh <<'EOF'
          rg -n TODO crates
          EOF
YAML
expect "another option leaves the operand a script" 0 other-option-then-script \
    "All 1 workflow job(s)" "!::error::"

# --------------------------------------------- round thirty-one, all probed
# Codex, on 5040c5e. `-c` beats `-s`: the string is the script and the heredoc
# is only its input. Probed — the body does not run.
write_workflow dash-s-and-c ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          bash -sc 'cat >/dev/null' <<'SCRIPT'
          rg -n TODO crates
          SCRIPT
YAML
expect "-c beats -s, so the body is data" 0 dash-s-and-c \
    "All 1 workflow job(s)" "!::error::"

# A wrapper hides the shell, and the body still reaches it.
write_workflow wrapped-stdin-shell ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          timeout 30 bash <<'SCRIPT'
          rg -n TODO crates
          SCRIPT
YAML
expect "a wrapped shell still reads its heredoc" 1 wrapped-stdin-shell \
    "build" "ripgrep"

# ...and so does a shell that is not the first command of the line.
write_workflow piped-stdin-shell ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: |
          echo ignored | bash <<'SCRIPT'
          rg -n TODO crates
          SCRIPT
YAML
expect "a shell later in a pipeline reads it too" 1 piped-stdin-shell \
    "build" "ripgrep"

# `bash < script` runs the file as the shell's stdin script.
write_workflow redirected-script ci.yml <<'YAML'
jobs:
  build:
    steps:
      - run: bash < ci/scripts/test-real-adapter-deps.sh
YAML
expect "a script redirected into a shell is followed" 1 redirected-script \
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
