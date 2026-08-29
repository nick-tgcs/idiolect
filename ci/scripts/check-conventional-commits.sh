#!/usr/bin/env bash
# Every non-merge commit a PR adds to its base must have a Conventional Commits
# subject. Called by the `conventional-commits` job in pr-validation.yml.
#
#   usage: check-conventional-commits.sh <base-ref> [head-ref]
#
# The base ref is the PR's OWN base, not `main`. This repo is GitFlow: `develop`
# runs ahead of `main`, so scanning `main...HEAD` made every feature PR inherit
# every develop-only commit. On 2026-08-28 one non-conforming subject on develop
# (`Surface helper process failures (#95)`) therefore failed every open feature
# PR, none of which could do anything about it.
#
# Lives in a script rather than inline in the workflow so it can be tested —
# see test-conventional-commits.sh.
set -uo pipefail

BASE="${1:-}"
HEAD_REF="${2:-HEAD}"

if [ -z "$BASE" ]; then
    echo "usage: $(basename "$0") <base-ref> [head-ref]" >&2
    exit 2
fi

# A gate that cannot run must never read as a gate that found nothing. An
# unfetched or misspelled base would otherwise yield an empty commit list, which
# is indistinguishable from a PR whose every subject is fine.
if ! git rev-parse --verify --quiet "$BASE" >/dev/null; then
    echo "::error::base ref '$BASE' does not resolve — the commit check could not run"
    exit 1
fi

# `..` and NOT `...`, and this is load-bearing rather than stylistic. HEAD is the
# PR merge ref, which GitHub rebuilds only when the PR is pushed to or retargeted,
# while `origin/<base>` is fetched fresh when the job runs. So the base routinely
# holds commits that HEAD does not, and `...` — being the symmetric difference —
# would pull those in and recreate the 2026-08-28 outage exactly. `..` asks the
# question actually being asked: what does this PR ADD to its base?
#
# --no-merges skips the merge commit GitHub synthesises for the PR ref (subject
# "Merge <sha> into <sha>"), which isn't a real commit and never conforms.
#
# `%H%x20%s`, not plain `%s`: a commit is allowed an EMPTY subject, and with a
# bare `%s` that is an empty line, indistinguishable from the blank line that a
# here-string makes out of an empty range. Prefixed with the hash, every real
# commit is a non-empty line and an empty subject is still caught.
CONVENTIONAL='^(feat|fix|docs|style|refactor|perf|test|chore|build|ci|revert)(\(.+\))?: .+'

if ! commits="$(git log --no-merges --format='%H%x20%s' "$BASE..$HEAD_REF")"; then
    # Checked explicitly: read from a process substitution or a pipe, a failing
    # `git log` produces no lines, the loop body never runs, and the script
    # reports success — the very silent pass the base-ref guard above exists to
    # prevent.
    echo "::error::could not list commits in '$BASE..$HEAD_REF' — the commit check could not run"
    exit 1
fi

offenders=0
while IFS= read -r line; do
    # Only an empty range reaches this, as a single blank line from the
    # here-string; every real commit carries its hash.
    [ -n "$line" ] || continue
    subject="${line#* }"
    if ! printf '%s' "$subject" | grep -qE "$CONVENTIONAL"; then
        echo "::error::Commit message does not follow conventional commits format: $subject"
        offenders=$((offenders + 1))
    fi
done <<<"$commits"

if [ "$offenders" -ne 0 ]; then
    echo "$offenders commit subject(s) above need rewording (git rebase -i, then force-push)."
    exit 1
fi

echo "All commit messages follow conventional commits format."
