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
# --------------------------------------------------------- the PR TITLE is the
# subject that actually lands. `squash_merge_commit_title` is COMMIT_OR_PR_TITLE
# and merges here are squashes, so a multi-commit PR puts its TITLE on the base
# branch — text no commit carried and the range mode never saw. That is how
# `8cfc392 Surface helper process failures (#95)` reached develop. Judging only
# the range polices text that is thrown away.
for title in \
    'Surface helper process failures (#95)' \
    'WIP' \
    'test(android)+docs: compound type' \
    'feat(): empty scope' \
    'feat:' \
    ''; do
    output="$(run_check "$REPO" --title "$title")"
    status=$?
    if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'PR title does not follow'; then
        ok "title rejected: '${title:-<empty>}'"
    else
        fail "PR title '$title' must be rejected (status=$status):
$output"
    fi
done

for title in 'fix(asr): halve nothing' 'chore(deps): bump uuid from 1.24.0 to 1.26.0' 'ci: gate the title too'; do
    output="$(run_check "$REPO" --title "$title")"
    status=$?
    if [ $status -eq 0 ]; then
        ok "title accepted: '$title'"
    else
        fail "PR title '$title' conforms and must pass (status=$status):
$output"
    fi
done

# `--title` with no argument is a caller bug, not a clean title.
output="$(run_check "$REPO" --title)"
status=$?
if [ $status -eq 2 ] && printf '%s' "$output" | grep -q 'usage:'; then
    ok "--title with no subject is a usage error, not a pass"
else
    fail "a missing --title argument must not read as a conforming title (status=$status):
$output"
fi

# --------------------------------- the count decides WHICH subject is judged
# COMMIT_OR_PR_TITLE takes the squash subject from the single commit when a PR
# has exactly one, and from the title only when it has more. Gating the title of
# a one-commit PR would block it over text that never reaches the branch — while
# the commit that DOES reach it was already judged by the range mode.
output="$(run_check "$REPO" --title 'Surface helper process failures (#95)' 1)"
status=$?
if [ $status -eq 0 ] && printf '%s' "$output" | grep -q 'Single-commit PR'; then
    ok "a one-commit PR's title is not gated, and says why"
else
    fail "a one-commit PR takes its squash subject from the commit (status=$status):
$output"
fi

output="$(run_check "$REPO" --title 'fix(asr): fine either way' 1)"
status=$?
if [ $status -eq 0 ]; then
    ok "a one-commit PR with a conforming title also passes"
else
    fail "a conforming title must never fail (status=$status):
$output"
fi

# The rejection has to explain itself. A multi-commit PR's title is gated on a
# merge method that might not be chosen — merge commits and rebase merges are
# both enabled here and neither uses the title — so an author who hits this is
# owed the reason, or the block reads as arbitrary.
output="$(run_check "$REPO" --title 'WIP' 3)"
if printf '%s' "$output" | grep -qi 'squash' && printf '%s' "$output" | grep -qi 'merge method'; then
    ok "the title rejection says which merge method makes the title land"
else
    fail "a conservative block must say why it is conservative:
$output"
fi

# Guard the guard: the skip above proves nothing unless that same title is
# rejected when the count says the title IS what lands.
for count in 2 3 12; do
    output="$(run_check "$REPO" --title 'Surface helper process failures (#95)' "$count")"
    status=$?
    if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'PR title does not follow'; then
        ok "with $count commits the title is judged"
    else
        fail "a multi-commit PR's title becomes the squash subject and must be judged (status=$status):
$output"
    fi
done

# An unknown count must NOT buy a skip. The title mode exists for the case the
# range mode cannot see, so anything it cannot read as exactly 1 is checked.
for count in "" 0 abc " 1" "1x" -1; do
    output="$(run_check "$REPO" --title 'Surface helper process failures (#95)' "$count")"
    status=$?
    if [ $status -ne 0 ] && printf '%s' "$output" | grep -q 'PR title does not follow'; then
        ok "count '${count:-<empty>}' is not a one-commit PR, so the title is judged"
    else
        fail "an unreadable commit count must fail closed (count='$count', status=$status):
$output"
    fi
done

# ------------------------------------------------- grandfathered commit SHAs
# `develop` is force-push-proof (protect-develop: non_fast_forward, empty bypass
# list), so a subject already merged there can never be reworded and would fail
# the develop -> main release PR forever. Such commits are skipped BY SHA.
#
# Everything below is HERMETIC: a fixture repository, and a COPY of the check
# script with the fixture's own SHA inserted into the list. Nothing here reads
# this repository's history or its configured entries.
#
# That is not fastidiousness, it is the fix for two P1s. This file runs as a
# step in the conventional-commits job, which gates every PR, so any assertion
# about live history fails unrelated PRs the moment that history changes —
# whether by the release landing (the entry leaves `main..develop`) or by
# anything non-conforming reaching `develop`. Either way it is the 2026-08-28
# outage rebuilt inside the test written to guard the fix for it. Whether the
# real release range passes is a question the release PR's own CI answers.
#
# Injecting into a COPY rather than reading an override keeps the production
# script seamless: there is no environment variable or flag by which anyone
# could add a SHA to the live gate.
GF="$WORK/grandfather"
git init -q -b main "$GF"
git -C "$GF" config user.email test@example.com
git -C "$GF" config user.name Test
git -C "$GF" commit -q --allow-empty -m 'feat: base'
git -C "$GF" checkout -q -b topic
git -C "$GF" commit -q --allow-empty -m 'Surface helper process failures (#95)'
FIXTURE_SHA="$(git -C "$GF" rev-parse HEAD)"
# A second commit with the SAME subject and a different SHA, to prove the match
# is on the hash.
git -C "$GF" commit -q --allow-empty -m 'Surface helper process failures (#95)'
TWIN_SHA="$(git -C "$GF" rev-parse HEAD)"

# Baseline: unmodified, neither commit is listed, so both are reported. Without
# this the skip below could be the gate ignoring the fixture for some other
# reason entirely.
output="$(run_check "$GF" main)"
status=$?
if [ $status -ne 0 ] && [ "$(printf '%s\n' "$output" | grep -c '::error::Commit message')" -eq 2 ]; then
    ok "unlisted commits are reported — the fixture range is judged normally"
else
    fail "the fixture must fail the unmodified gate, or the skip proves nothing (status=$status):
$output"
fi

# The copy, with the fixture's SHA appended to the list. Inserted after the
# opening line rather than substituted for an existing entry, so these cases
# keep working when the production list is emptied — which is exactly what
# happens once a release retires the last entry.
COPY="$WORK/check-with-fixture.sh"
awk -v sha="$FIXTURE_SHA" '{ print } /^GRANDFATHERED_SHAS="$/ { print sha }' "$CHECK" >"$COPY"
chmod +x "$COPY"
if ! grep -qx "$FIXTURE_SHA" "$COPY"; then
    fail "the fixture SHA was not inserted into the copy — the anchor line changed?"
elif ! bash -n "$COPY" 2>/dev/null; then
    fail "the generated copy is not valid shell — the insertion landed in the wrong place"
else
    ok "a copy of the check script carries the fixture SHA"

    output="$(cd "$GF" && "$COPY" main 2>&1)"
    status=$?
    if [ $status -ne 0 ] && printf '%s' "$output" | grep -q "skipping grandfathered commit $FIXTURE_SHA"; then
        ok "a listed SHA is skipped, and the skip is announced"
    else
        fail "$FIXTURE_SHA is listed and must be skipped and announced (status=$status):
$output"
    fi

    # The twin carries the identical subject: it must still be reported, and it
    # is also what keeps the case above from passing on a gate that skips
    # everything.
    if printf '%s' "$output" | grep -q '::error::Commit message'; then
        ok "the twin with the same subject is still reported — matching is by SHA"
    else
        fail "only $FIXTURE_SHA is listed; $TWIN_SHA shares its subject and must still fail:
$output"
    fi

    # And with BOTH listed the range is clean, which is the release case in
    # miniature — with no dependence on when the real release happens.
    COPY2="$WORK/check-with-both.sh"
    awk -v a="$FIXTURE_SHA" -v b="$TWIN_SHA" '{ print } /^GRANDFATHERED_SHAS="$/ { print a; print b }' "$CHECK" >"$COPY2"
    chmod +x "$COPY2"
    output="$(cd "$GF" && "$COPY2" main 2>&1)"
    status=$?
    if [ $status -eq 0 ] && printf '%s' "$output" | grep -q '2 grandfathered'; then
        ok "a fully grandfathered range passes and counts what it skipped"
    else
        fail "with both SHAs listed the range must pass and say how many it skipped (status=$status):
$output"
    fi
fi

# The production list is never executed here, but a typo in it would be silent
# until a release. Checked statically, without git: every entry is a full SHA.
bad_entries="$(awk '/^GRANDFATHERED_SHAS="$/ { inlist = 1; next }
                    inlist && /^"$/ { inlist = 0 }
                    inlist && $0 !~ /^[0-9a-f]{40}$/ && NF { print }' "$CHECK")"
if [ -z "$bad_entries" ]; then
    ok "every configured grandfather entry is a full 40-character SHA"
else
    fail "malformed grandfather entries (a short SHA or stray text never matches anything):
$bad_entries"
fi

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
