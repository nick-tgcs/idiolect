#!/usr/bin/env bash
# Every non-merge commit a PR adds to its base must have a Conventional Commits
# subject. Called by the `conventional-commits` job in pr-validation.yml.
#
#   usage: check-conventional-commits.sh <base-ref> [head-ref] \
#              [head-branch] [head-repo] [this-repo]
#
# <head-ref> is the rev whose commits are scanned (`HEAD`, the PR merge ref, in
# CI). The last three describe the PR itself — things the merge ref cannot
# supply — and are read ONLY by the develop -> main release exemption below.
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
# Absent by default: with no head branch named, no PR can match the release
# shape, so the exemption stays shut for every caller that does not opt in.
HEAD_BRANCH="${3:-}"
HEAD_REPO="${4:-}"
THIS_REPO="${5:-}"

if [ -z "$BASE" ]; then
    echo "usage: $(basename "$0") <base-ref> [head-ref] [head-branch] [head-repo] [this-repo]" >&2
    exit 2
fi

# A gate that cannot run must never read as a gate that found nothing. An
# unfetched or misspelled base would otherwise yield an empty commit list, which
# is indistinguishable from a PR whose every subject is fine.
if ! git rev-parse --verify --quiet "$BASE" >/dev/null; then
    echo "::error::base ref '$BASE' does not resolve — the commit check could not run"
    exit 1
fi

# The develop -> main release PR is not re-scanned. Two facts make that safe,
# and both are enforced elsewhere in this same workflow: `main` only ever
# receives `develop` (the main-source-guard job fails any other head), and every
# commit on develop was already judged by this gate on the PR that put it there.
# Re-judging all of develop at release time only re-runs work already done — and
# it is exactly the redundancy behind the 2026-08-28 outage. It is also
# unfixable when it fires: `8cfc392 Surface helper process failures (#95)` is on
# develop with a non-conforming subject, and the protect-develop ruleset forbids
# the force-push that rewording it would need, for everyone, with an empty
# bypass list. A gate whose only remedy is to disable a branch protection is not
# a gate worth having.
#
# Every half of the shape is required, and the head halves are why this takes
# arguments beyond the range at all. In CI the rev to scan is the merge ref,
# `HEAD`, which names neither a branch nor a repository — so both are passed
# separately, as data, from `github.head_ref` and the PR's head repo.
#
# Keying on the base alone would fire on ANY PR into main, leaving
# main-source-guard as the only thing between an arbitrary branch and an
# unscanned merge to main. And keying on the branch NAME alone repeats the hole
# that job documents: a fork's branch may also be called `develop`. So the
# exemption matches only OUR develop, exactly as main-source-guard does.
#
# Exact equality throughout. The `origin/` strip applies to the BASE alone,
# because that prefix is something the workflow adds; `github.head_ref` never
# carries it, and stripping it there would exempt a branch actually named
# `origin/develop`. So `mainline`, `main-2` and `upstream/main` stay ordinary
# bases, and `developer`, `develop-2` and `origin/develop` stay ordinary heads.
#
# The repo identity must be PRESENT, not merely equal: defaulted to empty,
# `$HEAD_REPO = $THIS_REPO` is two blanks matching, which would exempt any
# caller that named a develop head and no repository at all.
RELEASE_BASE=main
RELEASE_HEAD=develop
if [ "${BASE#origin/}" = "$RELEASE_BASE" ] &&
    [ "$HEAD_BRANCH" = "$RELEASE_HEAD" ] &&
    [ -n "$THIS_REPO" ] && [ "$HEAD_REPO" = "$THIS_REPO" ]; then
    echo "Base is '$BASE' and head is '$HEAD_BRANCH' — the release PR. Its commits were"
    echo "each judged on the PR that merged them to $RELEASE_HEAD; not re-scanning here."
    exit 0
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
