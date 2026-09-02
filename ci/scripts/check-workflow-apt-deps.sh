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

# Resolves on any Ubuntu or Debian with usable package lists, and is entirely
# independent of the input. If THIS cannot be found, the lists are missing or
# empty and every lookup below would report "unavailable" — an empty answer
# dressed up as a unanimous verdict.
CONTROL_PACKAGE="coreutils"

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

if [ -z "$(candidate_of "$CONTROL_PACKAGE")" ]; then
    echo "::error::control package '$CONTROL_PACKAGE' does not resolve — the workflow dependency check could not run"
    echo "apt package lists are missing or empty. Run 'sudo apt-get update' first;"
    echo "without them every package below would look unavailable."
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

while IFS= read -r workflow; do
    # The list above is newline-terminated, so the here-string yields a final
    # empty line that is not a path.
    [ -n "$workflow" ] || continue

    lineno=0
    pending=""
    pending_line=0

    while IFS= read -r raw; do
        lineno=$((lineno + 1))

        # A `run:` block may split one command across lines with a trailing
        # backslash. Joining them first is what stops the packages on the
        # continuation lines from going unexamined — silent under-coverage
        # reads exactly like a clean result.
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

        case "$line" in
        *"apt-get install"* | *"apt install"*) ;;
        *) continue ;;
        esac

        # Everything before `install` is the invocation (sudo, apt-get, flags);
        # everything after is the package list, up to the first shell operator.
        args="${line#*install}"

        # `${{ matrix.pkg }}` is ONE package name, and splitting on whitespace
        # makes it three tokens — of which the middle one carries no `$` and is
        # duly looked up as a package, does not resolve, and fails a correct
        # workflow. Squeezing the spaces out of each expression first keeps it a
        # single token for the interpolation check below. A false red here would
        # block every PR, which is how a gate gets switched off.
        args="$(printf '%s' "$args" | sed ':a;s/\(\${{[^}]*\) \([^}]*}}\)/\1\2/;ta')"

        # A shell operator ends the package list, but not necessarily the line:
        # `apt-get install -y a && apt-get install -y b` holds two of them, and
        # stopping at the operator would drop the second without saying so.
        # Resuming at the next bare `install` picks it up, while `&& echo
        # installed` stays off, since `installed` is not `install`.
        in_packages=true
        for token in $args; do
            if [ "$in_packages" = false ]; then
                [ "$token" = "install" ] && in_packages=true
                continue
            fi
            case "$token" in
            '&&' | '||' | ';' | '|' | '>' | '>>')
                in_packages=false
                continue
                ;;
            -*)
                continue
                ;;
            *'$'* | *'{{'*)
                # Announced, never silent: a package name assembled at runtime
                # cannot be resolved here, and a skip nobody can see is a skip
                # nobody can audit.
                echo "notice: $workflow:$pending_line names a package through a variable, not checked: $token"
                unresolvable=$((unresolvable + 1))
                continue
                ;;
            esac

            checked=$((checked + 1))
            if [ -z "$(candidate_of "$token")" ]; then
                echo "::error::$workflow:$pending_line installs a package apt cannot resolve: $token"
                missing=$((missing + 1))
            fi
        done
    done <"$workflow"

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
