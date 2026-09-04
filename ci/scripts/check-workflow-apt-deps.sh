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
# and the development machine are the same distribution (ubuntu-noble), so apt
# answers here exactly what it will answer there. `apt-cache policy` is asked
# first because it is cheap; where it reports no candidate the install itself
# is simulated, since a VIRTUAL name has no candidate of its own and only apt
# can say whether one package provides it or several do.
#
# The two halves are split by which problem they solve. Finding the package
# names means lexing shell and parsing YAML, and both are delegated to the
# standard library in workflow_apt_deps.py — see its header for why. What is
# left here is apt: whether it can be consulted at all, and what it says.
#
# LIMITATION, deliberate: a package name carrying a version, architecture or
# target-release suffix (`pkg=1.2`, `pkg:amd64`, `pkg/stable`) is looked up
# verbatim and would be reported unavailable. No workflow uses those forms; if
# one ever needs to, strip the suffix in the scanner rather than working around
# it in the workflow.
#
# Lives in a script rather than inline in a workflow so it can be tested —
# see test-workflow-apt-deps.sh.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCANNER="$SCRIPT_DIR/workflow_apt_deps.py"

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

if ! command -v python3 >/dev/null 2>&1; then
    echo "::error::python3 not found — the workflow dependency check could not run"
    exit 1
fi

if [ ! -f "$SCANNER" ]; then
    echo "::error::$SCANNER is missing — the workflow dependency check could not run"
    exit 1
fi

if ! python3 -c "import yaml" 2>/dev/null; then
    echo "::error::PyYAML not available — the workflow dependency check could not run"
    echo "Install python3-yaml; without it the workflow files cannot be parsed and"
    echo "every package in them would go unexamined."
    exit 1
fi

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

files=()
for target in "$@"; do
    if [ -d "$target" ]; then
        # maxdepth 1 because GitHub only reads workflows directly in this
        # directory; a nested .yml is not a workflow.
        found="$(find "$target" -maxdepth 1 -type f \( -name '*.yml' -o -name '*.yaml' \) | sort)"
        if [ -z "$found" ]; then
            # An empty directory is indistinguishable from a directory of clean
            # workflows once the scan has run, so it is caught first.
            echo "::error::no workflow files in '$target' — the workflow dependency check could not run"
            exit 1
        fi
        while IFS= read -r one; do
            [ -n "$one" ] && files+=("$one")
        done <<<"$found"
    elif [ -f "$target" ]; then
        files+=("$target")
    else
        echo "::error::'$target' does not exist — the workflow dependency check could not run"
        exit 1
    fi
done

if ! records="$(python3 "$SCANNER" "${files[@]}")"; then
    echo "::error::the scanner failed — the workflow dependency check could not run"
    exit 1
fi

# One `apt-cache` process per DISTINCT package rather than per occurrence: the
# workflows name 149 packages between them but only 14 different ones, and a
# candidate cannot change within a single run.
declare -A APT_CANDIDATE

# `resolve_candidate <package>` sets CANDIDATE to the candidate version, or to
# the empty string if apt cannot resolve the name. It assigns to a global rather
# than printing, because a `$(...)` call would run — and discard — the cache in a
# subshell, leaving one process per occurrence after all.
CANDIDATE=""
resolve_candidate() {
    local package="$1"

    if [ -n "${APT_CANDIDATE[$package]+set}" ]; then
        CANDIDATE="${APT_CANDIDATE[$package]}"
        return
    fi

    CANDIDATE="$(apt-cache policy "$package" 2>/dev/null |
        sed -n 's/^  Candidate: //p' |
        grep -v '^(none)$')"

    if [ -z "$CANDIDATE" ]; then
        # A VIRTUAL package has no candidate of its own: `libz-dev` is a name
        # `zlib1g-dev` PROVIDES, and `apt-cache policy` reports
        # `Candidate: (none)` for it while `apt-get install libz-dev` takes it
        # without complaint. apt accepts such a name when exactly one package
        # provides it and refuses when several do, which is a judgement only
        # apt makes — so the install itself is asked, in simulation.
        #
        # Only from here, and only in this direction: this branch can turn a
        # rejection into an acceptance and never the reverse, so the cost of
        # asking is bounded to the names already about to be reported. `--`
        # because a package name is not an option, whatever it starts with.
        if apt-get -s install -- "$package" >/dev/null 2>&1; then
            CANDIDATE="provided by another package"
        fi
    fi

    APT_CANDIDATE["$package"]="$CANDIDATE"
}

missing=0
checked=0
unresolvable=0

while IFS=$'\t' read -r kind path lineno value; do
    [ -n "$kind" ] || continue

    case "$kind" in
    PKG)
        checked=$((checked + 1))
        resolve_candidate "$value"
        if [ -z "$CANDIDATE" ]; then
            echo "::error::$path:$lineno installs a package apt cannot resolve: $value"
            missing=$((missing + 1))
        fi
        ;;
    NOTICE)
        # Announced, never silent: a skip nobody can see is a skip nobody can
        # audit.
        echo "notice: $path:$lineno $value"
        unresolvable=$((unresolvable + 1))
        ;;
    *)
        echo "::error::unrecognised record from the scanner: $kind"
        exit 1
        ;;
    esac
done <<<"$records"

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
