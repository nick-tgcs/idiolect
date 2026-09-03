#!/usr/bin/env bash
# Resolves every apt package named in the repository's CI definitions against
# the package lists of the machine running this script.
#
#   usage: check-workflow-apt-deps.sh [path ...]   (files or workflow directories)
#
# A workflow's `apt-get install` line is code that nothing compiles, nothing
# lints and nothing type-checks. It is proved wrong only by running the job that
# holds it — so a job that runs rarely can carry a broken one indefinitely:
#
#   release-main.yml   ran 3 times (2026-06-24, 2026-07-21, 2026-09-02) and
#                      failed all 3 on `E: Unable to locate package
#                      libfcitx5-dev`, before compiling anything. `publish-edge`
#                      `needs:` that build, so it never ran once and the rolling
#                      `edge` prerelease it exists to publish has never existed.
#   scheduled.yml      failed the same way on 10 consecutive nights.
#
# Both were copied from a job whose package list was correct at the time and
# then drifted; `libfcitx5-dev`, `libfcitx5qt-dev` and `libfcitx5qt1-dev` do not
# exist on ubuntu-noble, where the fcitx5 development headers are
# `libfcitx5core-dev`. Every workflow that passes already said `libfcitx5core-dev`
# — the information needed to catch this was in the repository the whole time,
# with nothing to compare it against.
#
# This is the comparison. It is sound rather than heuristic because the runner
# and the development machine are the same distribution (ubuntu-noble), so
# `apt-cache policy` answers here exactly what `apt-get install` will answer
# there.
#
# LIMITATION, deliberate: a package name carrying a version or architecture
# suffix (`pkg=1.2`, `pkg:amd64`) is looked up verbatim and would be reported
# unavailable. No workflow uses that form; if one ever needs to, strip the
# suffix here rather than working around it in the workflow.
#
# Lives in a script rather than inline in a workflow so it can be tested —
# see test-workflow-apt-deps.sh.
set -uo pipefail

# CI_README.md by default as well as the workflows: the workflows did not invent
# these package lists, they were copied from the README, which told contributors
# to install exactly the names that do not exist. A gate on one entrance is not
# a gate.
if [ "$#" -eq 0 ]; then
    set -- ".github/workflows" ".github/CI_README.md"
fi

if ! command -v apt-cache >/dev/null 2>&1; then
    echo "::error::apt-cache not found — the workflow dependency check could not run"
    echo "The workflows target ubuntu-latest, so this check needs a Debian-family"
    echo "machine to resolve their package names against."
    exit 1
fi

candidate_of() { # candidate_of <package> -> prints the candidate version, if any
    apt-cache policy "$1" 2>/dev/null |
        sed -n 's/^  Candidate: //p' |
        grep -v '^(none)$'
}

# The control has to be the ARCHIVE INDEXES, not a package. `apt-cache policy`
# with no arguments lists the package files apt is working from; with nothing
# fetched that is `/var/lib/dpkg/status` alone. A control PACKAGE cannot see
# that condition, because an INSTALLED package still reports a candidate out of
# the local dpkg database and the guard concludes all is well — while every
# dependency that is merely declared, not installed, is then reported as
# nonexistent. An empty answer dressed up as a unanimous verdict.
repository_indexes="$(apt-cache policy 2>/dev/null |
    grep -E '^[[:space:]]+[0-9]+[[:space:]]' |
    grep -vF '/var/lib/dpkg/status')"

if [ -z "$repository_indexes" ]; then
    echo "::error::apt has no repository indexes — the workflow dependency check could not run"
    echo "Only /var/lib/dpkg/status is available, so apt-cache is answering from the"
    echo "local package database and anything not already installed would look"
    echo "nonexistent. Run 'sudo apt-get update' first."
    exit 1
fi

workflows=""
for target in "$@"; do
    if [ -d "$target" ]; then
        # maxdepth 1 because GitHub only reads workflows directly in this
        # directory; a nested .yml is not a workflow.
        found="$(find "$target" -maxdepth 1 -type f \( -name '*.yml' -o -name '*.yaml' \) | sort)"
        if [ -z "$found" ]; then
            # An empty directory is indistinguishable from a directory of clean
            # workflows once the loop below has run, so it is caught first.
            echo "::error::no workflow files in '$target' — the workflow dependency check could not run"
            exit 1
        fi
        workflows="$workflows$found
"
    elif [ -f "$target" ]; then
        workflows="$workflows$target
"
    else
        echo "::error::'$target' does not exist — the workflow dependency check could not run"
        exit 1
    fi
done

missing=0
checked=0
unresolvable=0

# `judge_package <workflow> <line> <token>` — one candidate package name.
judge_package() {
    local workflow="$1" lineno="$2" token="$3"

    # Shell quoting is not part of the name: `"cmake"` installs `cmake`.
    token="${token#[\"\']}"
    token="${token%[\"\']}"
    [ -n "$token" ] || return

    case "$token" in
    -*) return ;;
    *'$'* | *'{{'* | *'('* | *')'*)
        # Announced, never silent: a package name assembled at runtime cannot be
        # resolved here, and a skip nobody can see is a skip nobody can audit.
        # The parentheses matter on their own: `$(cat pkgs.txt)` splits into
        # `$(cat` and `pkgs.txt)`, and only the first carries a `$`.
        echo "notice: $workflow:$lineno names a package through a variable, not checked: $token"
        unresolvable=$((unresolvable + 1))
        return
        ;;
    esac

    # A comma is deliberately NOT stripped. The shell passes it straight to apt,
    # which rejects it: `apt-get -s install cmake,` exits 100 with "Unable to
    # locate package cmake,". Removing it would hide exactly that typo.
    checked=$((checked + 1))
    if [ -z "$(candidate_of "$token")" ]; then
        echo "::error::$workflow:$lineno installs a package apt cannot resolve: $token"
        missing=$((missing + 1))
    fi
}

# `scan_line <workflow> <line-number> <text>` — judges one LOGICAL line: a single
# shell command line, already assembled from whatever YAML and shell spread it
# across several physical ones.
scan_line() {
    local workflow="$1" pending_line="$2" line="$3"
    local state cmd_prefix saw_apt saw_install parsed_install token word sep

    # Cheap pre-filter: a line that never says "apt" cannot hold an install
    # command, and this skips almost every line in the repository.
    case "$line" in
    *apt*) ;;
    *) return ;;
    esac

    # `${{ matrix.pkg }}` is ONE package name, and splitting on whitespace makes
    # it three tokens — of which the middle one carries no `$` and is duly looked
    # up as a package, does not resolve, and fails a correct workflow. Squeezing
    # the spaces out of each expression first keeps it a single token. A false
    # red here would block every PR, which is how a gate gets switched off.
    line="$(printf '%s' "$line" | sed ':a;s/\(\${{[^}]*\) \([^}]*}}\)/\1\2/;ta')"

    # Walked as a state machine rather than by matching the string "apt-get
    # install", because that string is not the only way to write it and not the
    # only way it appears:
    #
    #   - `apt-get [options] install pkg1 ...` is the documented syntax, so
    #     `apt-get --no-install-recommends install -y bad-pkg` holds no such
    #     substring and walked straight past the invalid package;
    #   - a `#` comment runs to end of line, so `install -y cmake # for the
    #     addon` had `#`, `for`, `the` and `addon` looked up as packages;
    #   - CI_README.md is prose, where "the apt install step" is a sentence and
    #     not a command, so an install starts only in COMMAND POSITION.
    state=idle
    # Command position: the start of the line, and again after every shell
    # operator. It survives the prefix material an invocation may carry —
    # sudo/env, their options, and `VAR=value` assignments — and dies on the
    # first token that is none of those.
    cmd_prefix=true
    # If a line names apt and names `install` but never reaches the package
    # list, the parser did not understand the form. Not parsing it is
    # acceptable; not saying so is not.
    saw_apt=false
    saw_install=false
    parsed_install=false

    case "$line" in
    *apt-get* | *' apt '*) saw_apt=true ;;
    esac
    case "$line" in
    *install*) saw_install=true ;;
    esac

    for token in $line; do
        case "$token" in
        *'$'* | *'{{'*)
            # An interpolated value may contain any of the characters below. It
            # is judged by state further down, never as punctuation.
            ;;
        "#"*)
            # A comment runs to the end of the line. Continuations were joined
            # above, so nothing after this is a command either.
            break
            ;;
        *[';&|<>']*)
            # Shell metacharacters need no whitespace around them, so ONE
            # whitespace-delimited token can hold the end of a word and the
            # operator that ends it: `bad-package>/dev/null`, `cmake&&`,
            # `cmake;`. What precedes the first metacharacter is still a word
            # the shell passes to apt; what follows belongs to the next command.
            word="${token%%[;&|<>]*}"
            sep="${token#"$word"}"
            if [ "$state" = packages ] && [ -n "$word" ]; then
                case "$word" in
                *[!0-9]*)
                    judge_package "$workflow" "$pending_line" "$word"
                    ;;
                *)
                    # All digits in front of a redirection is a file descriptor
                    # (`2>&1`), not a package.
                    case "$sep" in
                    [\<\>]*) ;;
                    *) judge_package "$workflow" "$pending_line" "$word" ;;
                    esac
                    ;;
                esac
            fi
            state=idle
            cmd_prefix=true
            continue
            ;;
        esac

        case "$state" in
        idle)
            [ "$cmd_prefix" = true ] || continue
            case "$token" in
            apt | apt-get) state=options ;;
            # Still an invocation prefix, so the command is yet to come.
            # `sudo -E apt-get ...` is documented sudo syntax and put an option
            # between the two.
            sudo | env | 'run:' | '-' | -* | *=*) ;;
            # Any other word is the command itself, or prose.
            *) cmd_prefix=false ;;
            esac
            ;;
        options)
            # Everything between `apt-get` and its subcommand is an option; only
            # `install` opens a package list.
            if [ "$token" = "install" ]; then
                state=packages
                parsed_install=true
            fi
            ;;
        packages)
            judge_package "$workflow" "$pending_line" "$token"
            ;;
        esac
    done

    if [ "$saw_apt" = true ] && [ "$saw_install" = true ] && [ "$parsed_install" = false ]; then
        echo "notice: $workflow:$pending_line looks like an apt install command but was not parsed as one — its packages were not checked"
        unresolvable=$((unresolvable + 1))
    fi
}

while IFS= read -r workflow; do
    # The list above is newline-terminated, so the here-string yields a final
    # empty line that is not a path.
    [ -n "$workflow" ] || continue

    lineno=0
    pending=""
    pending_line=0
    # `run: >` is a FOLDED scalar: GitHub joins the more-indented lines beneath
    # it into ONE command, so a package can sit on a line that names no command.
    # -1 means no fold is open; otherwise it holds the indentation of the `run:`
    # key, and the fold ends at the first blank line or the first line indented
    # no further than that key.
    fold_indent=-1
    fold_line=0
    fold_buf=""

    while IFS= read -r raw; do
        lineno=$((lineno + 1))

        indent="${raw%%[! ]*}"
        trimmed="${raw#"$indent"}"

        if [ "$fold_indent" -ge 0 ]; then
            if [ -n "$trimmed" ] && [ "${#indent}" -gt "$fold_indent" ]; then
                fold_buf="$fold_buf $trimmed"
                continue
            fi
            # The fold closed. Judge what it collected, then fall through and
            # handle THIS line as an ordinary one.
            scan_line "$workflow" "$fold_line" "$fold_buf"
            fold_indent=-1
            fold_buf=""
        fi

        # Only a `>` standing alone as the scalar indicator opens a fold;
        # `run: echo a > b` is a command that happens to redirect.
        case "$trimmed" in
        run:*)
            scalar="${trimmed#run:}"
            scalar="${scalar#"${scalar%%[! ]*}"}"
            case "$scalar" in
            '>' | '>-' | '>+')
                fold_indent="${#indent}"
                fold_line=$lineno
                fold_buf=""
                continue
                ;;
            esac
            ;;
        esac

        # A `run:` block may also split one command across lines with a trailing
        # backslash. Joining them is what stops the packages on the continuation
        # lines from going unexamined — silent under-coverage reads exactly like
        # a clean result.
        line="$raw"
        if [ -n "$pending" ]; then
            line="$pending $line"
        else
            pending_line=$lineno
        fi
        case "$line" in
        *\\)
            pending="${line%\\}"
            continue
            ;;
        esac
        pending=""

        scan_line "$workflow" "$pending_line" "$line"
    done <"$workflow"

    if [ "$fold_indent" -ge 0 ]; then
        # A fold that runs to the end of the file still holds a command.
        scan_line "$workflow" "$fold_line" "$fold_buf"
    fi

    if [ -n "$pending" ]; then
        # A `run:` block whose last line ends in a backslash. The join above
        # swallowed it, so say so rather than let it count as examined.
        echo "notice: $workflow ends inside a line continuation started at line $pending_line — its packages were not checked"
        unresolvable=$((unresolvable + 1))
    fi
done <<<"$workflows"

if [ "$checked" -eq 0 ]; then
    echo "::error::no apt packages found in the paths given — the workflow dependency check found nothing to check"
    exit 1
fi

if [ "$missing" -ne 0 ]; then
    echo
    echo "$missing package name(s) above do not exist on this distribution, so the job"
    echo "installing them fails at apt before it builds anything. Compare against a"
    echo "workflow job that passes — the correct names are already there."
    exit 1
fi

if [ "$unresolvable" -ne 0 ]; then
    echo "All $checked apt package(s) in $* resolve ($unresolvable not checkable, listed above)."
else
    echo "All $checked apt package(s) in $* resolve."
fi
