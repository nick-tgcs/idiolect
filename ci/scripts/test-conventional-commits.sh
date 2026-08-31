#!/usr/bin/env bash
# Tests for check-conventional-commits.sh — the commit-subject gate.
#
# The gate lives in a script rather than inline in pr-validation.yml precisely so
# it can be run red/green here. Its bugs are not hypothetical: on 2026-08-28 it
# scanned `origin/main...HEAD` on a GitFlow repo, so a non-conforming commit that
# was already on `develop` failed EVERY feature PR, whatever that PR's own
# commits said.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$SCRIPT_DIR/check-conventional-commits.sh"

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

# A repository shaped like this one: `main` behind `develop`, and a
# non-conforming subject sitting on `develop` that no feature PR can do anything
# about.
make_repo() {
    local root="$1"
    git init -q -b main "$root"
    git -C "$root" config user.email test@example.com
    git -C "$root" config user.name Test
    git -C "$root" commit -q --allow-empty -m 'feat: initial commit'

    git -C "$root" checkout -q -b develop
    git -C "$root" commit -q --allow-empty -m 'Surface helper process failures (#95)'

    git -C "$root" checkout -q -b feature
    git -C "$root" commit -q --allow-empty -m 'feat(ime): a conforming subject'
    git -C "$root" commit -q --allow-empty -m 'fix(asr): another conforming subject'
}

run_check() { # run_check <repo> <base> [head] -> exit code, output on stdout
    local repo="$1"
    shift
    (cd "$repo" && "$CHECK" "$@" 2>&1)
}

# Without this every negative case below would pass on exit 127 — the subject
# missing looks exactly like the subject rejecting bad input.
if [ ! -x "$CHECK" ]; then
    printf 'FAIL: %s is missing or not executable — no case below would mean anything\n' "$CHECK" >&2
    exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# ---------------------------------------------------------------- the real bug
REPO="$WORK/gitflow"
make_repo "$REPO"

output="$(run_check "$REPO" develop)"
if [ $? -eq 0 ]; then
    ok "a feature branch is judged on its OWN commits when scanned against its base"
else
    fail "scanning against the PR's base must ignore commits already on that base
$output"
fi

output="$(run_check "$REPO" main)"
status=$?
if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'Surface helper process failures'; then
    ok "scanning against the wrong base still reports the base's own commit (the 2026-08-28 outage)"
else
    fail "expected the develop-only subject to be named when scanning from main (status=$status):
$output"
fi

# ------------------------------------------------------- it still catches yours
git -C "$REPO" commit -q --allow-empty -m 'WIP'
git -C "$REPO" commit -q --allow-empty -m 'test(android)+docs: compound type'
output="$(run_check "$REPO" develop)"
status=$?
if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'WIP'; then
    ok "a non-conforming subject on the branch itself fails"
else
    fail "a bare 'WIP' subject must fail the gate and be named (status=$status):
$output"
fi

# EVERY offender is named. The inline version put `exit 1` inside a piped
# `while`, so it stopped at the first one and you learned about the next only
# after rewording, pushing, and waiting for CI again.
if [ "$(printf '%s\n' "$output" | grep -c '::error::Commit message')" -eq 2 ]; then
    ok "every offending subject is reported, not just the first"
else
    fail "expected 2 reported offenders, got:
$output"
fi

# ------------------------------------------------------- a gate that cannot run
# must fail loudly. A base ref that was never fetched used to yield an empty
# commit list, which reads exactly like 'nothing to complain about'.
# Asserted on the SPECIFIC diagnostic, not merely on a non-zero exit: the
# generic "could not list commits" path below would also fail here, so without
# this the dedicated base-ref guard could be deleted with nothing noticing. A
# base that was never fetched is the likeliest operational failure of this gate
# and deserves to say so by name.
output="$(run_check "$REPO" never-fetched-branch)"
status=$?
if [ $status -ne 0 ] && printf '%s' "$output" | grep -q "base ref 'never-fetched-branch' does not resolve"; then
    ok "an unresolvable base ref fails loudly, naming the ref"
else
    fail "an unresolvable base must not read as a clean scan (status=$status):
$output"
fi

# ------------------------------------------------------------------- injection
# `github.base_ref` reaches the script as data. Git ref names may contain `;`,
# `$` and backticks, so a base ref must never be evaluated as shell code.
# Under $WORK, not a fixed /tmp path: a squatted name (or a directory, which
# `rm -f` will not remove) made this report an injection that never happened.
SENTINEL="$WORK/pwned"
EVIL="pwn;touch $SENTINEL"
output="$(run_check "$REPO" "$EVIL")"
status=$?
if [ -e "$SENTINEL" ]; then
    fail "a base ref containing ';' was executed as shell code"
elif [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'pwn;touch'; then
    # Reported verbatim as an unresolvable ref: it crossed into git as one
    # argument, and no part of it was taken as a command separator.
    ok "a base ref carrying shell metacharacters is treated as data"
else
    fail "the injection case proved nothing — the script must run and reject the ref (status=$status):
$output"
fi

# ------------------------------------------- `..` vs `...`, and `--no-merges`
# Both are single tokens whose loss is catastrophic, and a linear fixture cannot
# tell either of them apart from its mutation. This one can: the merge ref is
# built while the base is at C1, and the base THEN gains a non-conforming commit
# — which is the real timing, because GitHub rebuilds refs/pull/N/merge only on
# push or retarget while `origin/<base>` is fetched fresh when the job runs.
DIVERGED="$WORK/diverged"
git init -q -b develop "$DIVERGED"
git -C "$DIVERGED" config user.email test@example.com
git -C "$DIVERGED" config user.name Test
git -C "$DIVERGED" commit -q --allow-empty -m 'feat: base commit'
git -C "$DIVERGED" checkout -q -b feature
git -C "$DIVERGED" commit -q --allow-empty -m 'fix: the PR own work'
# The PR merge ref, built the way GitHub builds it: the feature branch merged
# INTO the base as the base stood at the time. Merging the other way round would
# be "Already up to date" and produce NO merge commit even with --no-ff, leaving
# the --no-merges assertion below with no merge commit to be wrong about.
git -C "$DIVERGED" checkout -q -b merge-ref develop
git -C "$DIVERGED" merge -q --no-ff -m "Merge pull request into develop" feature
# ...and only now does the base move on, with a subject the PR cannot fix.
git -C "$DIVERGED" checkout -q develop
git -C "$DIVERGED" commit -q --allow-empty -m 'Surface helper process failures (#95)'
git -C "$DIVERGED" checkout -q merge-ref

output="$(run_check "$DIVERGED" develop)"
status=$?
if [ $status -eq 0 ]; then
    ok "a base that moved on after the merge ref was built does not taint the PR (.. not ...)"
else
    fail "the base's later commits must not be scanned — '...' would reintroduce the outage (status=$status):
$output"
fi

# Guard the guard: an assertion that a string is absent proves nothing unless
# the string could have been there.
if [ "$(git -C "$DIVERGED" rev-list --merges --count develop..merge-ref)" -ne 1 ]; then
    fail "the fixture has no merge commit — the --no-merges assertion would be vacuous"
elif printf '%s' "$output" | grep -q 'Merge pull request'; then
    fail "the PR merge commit itself must be skipped (--no-merges):
$output"
else
    ok "the synthesised merge commit is not judged (--no-merges)"
fi

# --------------------------------------------------- subjects the regex rejects
REGEX="$WORK/regex"
git init -q -b main "$REGEX"
git -C "$REGEX" config user.email test@example.com
git -C "$REGEX" config user.name Test
git -C "$REGEX" commit -q --allow-empty -m 'feat: base'
git -C "$REGEX" checkout -q -b feature

expect_rejected() { # expect_rejected <subject-description> <subject>
    git -C "$REGEX" checkout -q -B probe main
    git -C "$REGEX" commit -q --allow-empty --allow-empty-message -m "$2"
    local out
    out="$(run_check "$REGEX" main)"
    if [ $? -ne 0 ]; then
        ok "rejected: $1"
    else
        fail "must be rejected ($1):
$out"
    fi
}

# An EMPTY subject is not a conforming one. It used to be skipped outright by a
# `[ -n "$commit" ] || continue` guard, so `git commit --allow-empty-message`
# walked straight through the gate.
expect_rejected "an empty subject" ""
# git normalises this to the empty subject above rather than preserving it, so
# it reaches the gate as the same input — recorded because "   " looks like it
# is exercising a separate branch and is not.
expect_rejected "a whitespace-only subject (normalised to empty)" "   "
expect_rejected "an empty scope" "feat(): a description"
expect_rejected "a type with no description" "feat:"
expect_rejected "a compound type" "test(android)+docs: compound type"
# NOTE: `: .+` vs `: .*` in the pattern is an equivalent mutant and deliberately
# unpinned. Telling them apart needs a subject ending in "feat: " — a trailing
# space — and `git log %s` strips trailing whitespace, so no commit can carry one.

# ------------------------------------------- the head ref argument is honoured
# Documented as the second parameter and used by nothing today, so nothing would
# notice it being ignored.
git -C "$REGEX" checkout -q -B other main
git -C "$REGEX" commit -q --allow-empty -m 'WIP not conventional'
output="$(run_check "$REGEX" main other)"
status=$?
if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'WIP not conventional'; then
    ok "the head ref argument selects what is scanned"
else
    fail "check <base> <head> must scan <head> (status=$status):
$output"
fi

# ------------------------------------- a range that cannot be listed fails loud
output="$(run_check "$REGEX" main no-such-head-ref)"
status=$?
if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'could not list commits'; then
    ok "a range git cannot list fails loudly instead of passing vacuously"
else
    fail "an unlistable range must not read as a clean scan (status=$status):
$output"
fi

# ------------------------------------------------- the develop -> main release PR
# `main` only ever receives develop (enforced by the main-source-guard job), and
# every commit on develop was already judged by this gate on the PR that put it
# there. Re-judging the whole of develop at release time is the same redundancy
# that caused the 2026-08-28 outage: `8cfc392 Surface helper process failures
# (#95)` reached develop with a non-conforming subject, and no release PR can
# reword it — develop's protect-develop ruleset forbids the force-push, for
# everyone, with an empty bypass list. So a PR whose base is `main` is not
# scanned.
OURS=nick-tgcs/idiolect

# The exact shape CI passes for the release PR, with one part swapped per case.
# Everything below varies ONE argument from this, so each `ok` names the reason
# it was scanned rather than merely observing that it was.
release_check() { # release_check <base> <head-branch> <head-repo> <this-repo>
    run_check "$REPO" "$1" develop "$2" "$3" "$4"
}

output="$(release_check main develop "$OURS" "$OURS")"
status=$?
if [ $status -eq 0 ]; then
    ok "a release PR into main is not re-scanned"
else
    fail "base=main head=develop from this repo must be exempt (status=$status):
$output"
fi

# The workflow passes `origin/$BASE_REF`, never a bare branch name, so the
# exemption has to survive that spelling or it never fires in CI at all.
git -C "$REPO" update-ref refs/remotes/origin/main "$(git -C "$REPO" rev-parse main)"
output="$(release_check origin/main develop "$OURS" "$OURS")"
status=$?
if [ $status -eq 0 ]; then
    ok "the exemption recognises the origin/ prefixed spelling the workflow uses"
else
    fail "origin/main is the form CI passes and must be exempt too (status=$status):
$output"
fi

# Guard the guard: every pass above proves nothing unless that range really does
# contain a subject this gate would otherwise reject.
git -C "$REPO" branch -f mainline main
output="$(release_check mainline develop "$OURS" "$OURS")"
status=$?
if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'Surface helper process failures'; then
    ok "the exempted range is not empty — a non-main base still rejects it"
else
    fail "the release-PR cases would be vacuous: this range must contain an offender (status=$status):
$output"
fi

# EVERY half is required. Keyed on the base alone this would fire on any PR into
# main, leaving main-source-guard as the only thing between an arbitrary branch
# and an unscanned merge to main.
output="$(release_check main feature "$OURS" "$OURS")"
status=$?
if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'Surface helper process failures'; then
    ok "base=main with a head that is not develop is still scanned"
else
    fail "the exemption must need every half of the release shape (status=$status):
$output"
fi

# A FORK's branch may also be called `develop` — the hole main-source-guard
# documents and closes with a head-repo check. Without the same check here, a
# fork could put unscanned commits behind the release exemption.
output="$(release_check main develop attacker/idiolect "$OURS")"
status=$?
if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'Surface helper process failures'; then
    ok "a fork branch named develop is not the release PR"
else
    fail "the exemption must require OUR develop, not any repo's (status=$status):
$output"
fi

# Defaulted to empty, `$HEAD_REPO = $THIS_REPO` is two blanks matching. Absent
# repo identity must read as "not the release PR", not as "same repo".
output="$(release_check main develop "" "")"
status=$?
if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'Surface helper process failures'; then
    ok "an absent repo identity is not a matching one"
else
    fail "empty == empty must not exempt (status=$status):
$output"
fi

# ...and the whole thing is shut for the two-argument callers used everywhere
# else in this file, which name no head branch at all.
output="$(run_check "$REPO" main develop)"
status=$?
if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'Surface helper process failures'; then
    ok "with no head branch named, base=main is still scanned"
else
    fail "the exemption must be shut by default (status=$status):
$output"
fi

# ...and the base half is an equality test, not a prefix or a substring one.
git -C "$REPO" branch -f main-2 main
git -C "$REPO" update-ref refs/remotes/upstream/main "$(git -C "$REPO" rev-parse main)"
for base in mainline main-2 upstream/main; do
    output="$(release_check "$base" develop "$OURS" "$OURS")"
    status=$?
    if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'Surface helper process failures'; then
        ok "base '$base' is not treated as main"
    else
        fail "only an exact 'main' is exempt; '$base' must still be scanned (status=$status):
$output"
    fi
done

# The same for the head half. `origin/develop` is in this list deliberately: the
# base is `origin/`-stripped because the workflow adds that prefix, and applying
# the same strip to the head would exempt a branch actually named that.
for head in developer develop-2 upstream/develop origin/develop; do
    output="$(release_check main "$head" "$OURS" "$OURS")"
    status=$?
    if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'Surface helper process failures'; then
        ok "head '$head' is not treated as develop"
    else
        fail "only an exact 'develop' head is exempt; '$head' must still be scanned (status=$status):
$output"
    fi
done

# ------------------------------------------------------------ the summary count
output="$(run_check "$REPO" develop)"
if printf '%s' "$output" | grep -q '^2 commit subject(s) above need rewording'; then
    ok "the summary names how many subjects need rewording"
else
    fail "expected a summary of 2 offenders:
$output"
fi

# ------------------------------------------------------------------------ done
printf '\n%s passed, %s failed\n' "$PASSED" "$FAILED"
[ "$FAILED" -eq 0 ]
