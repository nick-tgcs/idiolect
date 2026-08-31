#!/usr/bin/env bash
# Conventional Commits enforcement for this repo, in two modes. Called by the
# `conventional-commits` job in pr-validation.yml.
#
#   usage: check-conventional-commits.sh <base-ref> [head-ref]
#          check-conventional-commits.sh --title <subject>
#
# The range mode judges every non-merge commit the PR ADDS to its base. The
# title mode judges the PR title, and it exists because the range mode alone
# never sees what actually lands:
#
#   `squash_merge_commit_title` on this repo is COMMIT_OR_PR_TITLE, and the
#   dependabot workflow (and every human squash) merges with `--squash`. For a
#   PR with more than one commit, GitHub therefore synthesises the base-branch
#   subject from the PR TITLE, after this gate has run and from text no commit
#   ever carried. `8cfc392 Surface helper process failures (#95)` is exactly
#   that: a one-parent squash commit whose subject was never judged, on a PR
#   whose own commits may well have conformed.
#
# So both are gated. Otherwise the gate polices text that gets thrown away and
# ignores the text that survives.
#
# The base ref is the PR's OWN base, not `main`. This repo is GitFlow: `develop`
# runs ahead of `main`, so scanning `main...HEAD` made every feature PR inherit
# every develop-only commit. On 2026-08-28 one non-conforming subject on develop
# (that same 8cfc392) therefore failed every open feature PR, none of which
# could do anything about it.
#
# Lives in a script rather than inline in the workflow so it can be tested —
# see test-conventional-commits.sh.
set -uo pipefail

CONVENTIONAL='^(feat|fix|docs|style|refactor|perf|test|chore|build|ci|revert)(\(.+\))?: .+'

usage() {
    echo "usage: $(basename "$0") <base-ref> [head-ref]" >&2
    echo "       $(basename "$0") --title <subject>" >&2
}

# ----------------------------------------------------------------- title mode
if [ "${1:-}" = "--title" ]; then
    if [ "$#" -lt 2 ]; then
        usage
        exit 2
    fi
    TITLE="${2-}"
    if ! printf '%s' "$TITLE" | grep -qE "$CONVENTIONAL"; then
        echo "::error::PR title does not follow conventional commits format: $TITLE"
        echo "The PR title becomes the squash subject on the base branch, so it is judged"
        echo "exactly like a commit subject. Reword the title; no rebase needed."
        exit 1
    fi
    echo "PR title follows conventional commits format."
    exit 0
fi

# ----------------------------------------------------------------- range mode
BASE="${1:-}"
HEAD_REF="${2:-HEAD}"

if [ -z "$BASE" ]; then
    usage
    exit 2
fi

# Commits that predate the title gate above and cannot be reworded: `develop` is
# force-push-proof (the protect-develop ruleset carries non_fast_forward with an
# EMPTY bypass list, so the rewrite is denied to everyone, owner included), and
# these are already merged. Left unlisted they fail the develop -> main release
# PR forever, whose base legitimately is `main`.
#
# This list retires itself. Once a release lands, the commit is on `main` too,
# so `main..develop` no longer contains it and the entry is dead — delete it
# then. It is deliberately NOT a blanket "skip the release PR": that would
# permanently stop judging the range where squash subjects actually land, which
# is the one place this gate has ever been blind.
#
# Matched by SHA and never by subject, so reusing the wording does not inherit
# the exemption.
GRANDFATHERED_SHAS="
8cfc392de6d0842c740c65de768f5050dc74f343
"

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
# commit is a non-empty line and an empty subject is still caught — and the hash
# is what the grandfather list matches on.
if ! commits="$(git log --no-merges --format='%H%x20%s' "$BASE..$HEAD_REF")"; then
    # Checked explicitly: read from a process substitution or a pipe, a failing
    # `git log` produces no lines, the loop body never runs, and the script
    # reports success — the very silent pass the base-ref guard above exists to
    # prevent.
    echo "::error::could not list commits in '$BASE..$HEAD_REF' — the commit check could not run"
    exit 1
fi

offenders=0
skipped=0
while IFS= read -r line; do
    # Only an empty range reaches this, as a single blank line from the
    # here-string; every real commit carries its hash.
    [ -n "$line" ] || continue
    sha="${line%% *}"
    subject="${line#* }"

    grandfathered=false
    for g in $GRANDFATHERED_SHAS; do
        if [ "$sha" = "$g" ]; then
            grandfathered=true
            break
        fi
    done
    if [ "$grandfathered" = true ]; then
        # Announced, never silent: a skip nobody can see is a gate nobody can
        # audit.
        echo "notice: skipping grandfathered commit $sha ($subject)"
        skipped=$((skipped + 1))
        continue
    fi

    if ! printf '%s' "$subject" | grep -qE "$CONVENTIONAL"; then
        echo "::error::Commit message does not follow conventional commits format: $subject"
        offenders=$((offenders + 1))
    fi
done <<<"$commits"

if [ "$offenders" -ne 0 ]; then
    echo "$offenders commit subject(s) above need rewording (git rebase -i, then force-push)."
    exit 1
fi

if [ "$skipped" -ne 0 ]; then
    echo "All commit messages follow conventional commits format ($skipped grandfathered)."
else
    echo "All commit messages follow conventional commits format."
fi
