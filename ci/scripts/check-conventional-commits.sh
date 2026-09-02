#!/usr/bin/env bash
# Conventional Commits enforcement for this repo, in two modes. Called by the
# `conventional-commits` job in pr-validation.yml.
#
#   usage: check-conventional-commits.sh <base-ref> [head-ref]
#          check-conventional-commits.sh --title <subject> [pr-commit-count]
#
# The range mode judges every non-merge commit the PR ADDS to its base. The
# title mode judges the PR title, and it exists because the range mode alone
# never sees what actually lands:
#
#   `squash_merge_commit_title` on this repo is COMMIT_OR_PR_TITLE, and the
#   dependabot workflow (and every human squash) merges with `--squash`. For a
#   PR with MORE THAN ONE commit, GitHub synthesises the base-branch subject
#   from the PR TITLE, after this gate has run and from text no commit ever
#   carried. `8cfc392 Surface helper process failures (#95)` is exactly that: a
#   one-parent squash commit whose subject was never judged, on a PR whose own
#   commits may well have conformed.
#
#   For a ONE-commit PR the same setting takes the subject from that commit
#   instead, which the range mode has already judged. Gating the title there
#   would block a PR over text that never reaches the branch.
#
# So exactly one of the two judges the subject that will land, and which one
# depends on the commit count. Between them the gate polices what survives
# rather than what gets thrown away.
#
# This repo also allows merge commits and rebase merges (`allow_merge_commit`
# and `allow_rebase_merge` are both true; `a88152e` is a real two-parent merge
# of PR #104). Under either, the title never lands: `merge_commit_title` is
# MERGE_MESSAGE, so the subject is "Merge pull request #N from ...", which is a
# merge commit and skipped by --no-merges anyway, and a rebase merge replays the
# commits themselves. A multi-commit PR's title is therefore gated on a merge
# method that MIGHT be chosen — and deliberately so: the method is picked at
# merge time by a human and is unknowable here, so the gate fails closed. The
# cost of being strict is editing a title, with no rebase and no force-push. The
# cost of being lax was 8cfc392: an unrewordable subject on a force-push-proof
# branch, and a day of red CI on every open PR. To make the gate exact instead
# of conservative, disable merge commits and rebase merges at the repository
# level — a policy decision, not something this script should assume.
#
# The base ref is the PR's OWN base, not `main`. This repo is GitFlow: `develop`
# runs ahead of `main`, so scanning `main...HEAD` made every feature PR inherit
# every develop-only commit. On 2026-08-28 one non-conforming subject on develop
# (that same 8cfc392) therefore failed every open feature PR, none of which
# could do anything about it.
#
# LIMITATION, deliberate and unclosable from here: this gate judges the DEFAULT
# squash subject, not the final one. Whoever merges can overwrite it — the merge
# dialog has an editable subject field, and `gh pr merge --squash` documents
# `-t, --subject text` — and doing so neither updates the PR nor re-runs
# validation, so a non-conforming subject can still land with this job green.
# Nothing that runs at PR time can prevent that; it needs enforcement where the
# commit arrives. The options are a repository ruleset with a commit-message
# pattern (metadata restrictions, which are org/plan gated and so may not be
# available on this user-owned repo) or routing squash merges through automation
# that sets the subject. Both are repository policy, not this script's to
# assume. Recorded so nobody reads a green check here as proof that the subject
# on the branch conforms — it is proof that the default one did.
#
# Lives in a script rather than inline in the workflow so it can be tested —
# see test-conventional-commits.sh.
set -uo pipefail

CONVENTIONAL='^(feat|fix|docs|style|refactor|perf|test|chore|build|ci|revert)(\(.+\))?: .+'

usage() {
    echo "usage: $(basename "$0") <base-ref> [head-ref]" >&2
    echo "       $(basename "$0") --title <subject> [pr-commit-count]" >&2
}

# ----------------------------------------------------------------- title mode
if [ "${1:-}" = "--title" ]; then
    if [ "$#" -lt 2 ]; then
        usage
        exit 2
    fi
    TITLE="${2-}"
    PR_COMMITS="${3-}"

    # Exactly one commit means GitHub takes the squash subject from that commit,
    # not from this title — and the range mode has already judged it. Anything
    # else, including a count that is missing or not a number, is checked: an
    # unknown count must not buy a skip, since the whole point of this mode is
    # the case the range mode cannot see.
    if [ "$PR_COMMITS" = "1" ]; then
        echo "Single-commit PR: the squash subject comes from that commit, which the"
        echo "range check judges directly. PR title not gated."
        exit 0
    fi

    if ! printf '%s' "$TITLE" | grep -qE "$CONVENTIONAL"; then
        echo "::error::PR title does not follow conventional commits format: $TITLE"
        echo "This PR has more than one commit, so if it is SQUASH-merged the base-branch"
        echo "subject is taken from this title. A merge commit or a rebase merge would not"
        echo "use it — but the merge method is chosen at merge time and cannot be known"
        echo "here, so the title has to conform either way. Reword the title; no rebase"
        echo "needed, and nothing to force-push."
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
# Retiring an entry: delete it only once THIS SHA is an ancestor of `main`.
#
#     git merge-base --is-ancestor <sha> origin/main && echo "safe to delete"
#
# "after the next release" is NOT the condition, and the difference is not
# academic. Only an ancestry-preserving merge puts this very commit on `main`; a
# squash or rebase release creates new SHAs, leaves the original in
# `main..develop`, and deleting the entry then fails every subsequent release PR.
# Both methods are enabled on this repo, and main-source-guard constrains the
# release PR's source branch, not how it is merged. Verified on fixtures: after
# a merge-commit release `main..develop` holds the commit 0 times and it is an
# ancestor of main; after a squash release it holds it once and is not.
#
# The list is deliberately NOT a blanket "skip the release PR": that would
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
