#!/usr/bin/env bash
# Checks that every workflow job installs the command-line tools the scripts it
# runs actually need.
#
#   usage: check-workflow-tool-deps.sh [path ...]   (files or workflow directories)
#
# The sibling check (check-workflow-apt-deps.sh) asks whether the packages a
# workflow names EXIST. This one asks the other half of the same question:
# whether the packages a workflow needs are NAMED. A job can install nothing but
# real packages and still die, because the tool it forgot is a tool apt was
# never asked about — and apt cannot report a name that is not there.
#
# That is not hypothetical here. `ripgrep` is not part of the ubuntu-24.04
# runner image, and three ci/scripts call `rg`. Two jobs install it; the three
# in scheduled.yml run those same scripts and do not. Nothing caught it, because
# every package those jobs DO name resolves perfectly.
#
# A job "needs" a tool if one of its `run:` steps uses it directly, or runs a
# repo script that uses it, or runs a script that runs such a script — the last
# case being real: test-all.sh calls test-coverage-map.sh, so a job invoking
# only test-all.sh still needs ripgrep.
#
# LIMITATION: the gate reads a job as a set, not a sequence, so it cannot tell
# an install that comes AFTER the step needing it, nor an install behind an
# `if:` that turns out false. Both fail loudly on the job's first run now that
# the scripts refuse to start without their tool, which is the property that
# made this worth leaving out.
#
# LIMITATION, deliberate: the table below holds the tools this repository has
# actually been bitten by, not every command a script might call. A general
# "which binaries does this shell script invoke" analysis would have to model
# the runner image's entire preinstalled set to know which ones matter, and
# would report `cargo`, `sed` and `bash` forever. Add a row when a workflow
# needs a tool the image does not ship; the check is only as wide as its table
# and says so in its output.
#
# Lives in a script rather than inline in a workflow so it can be tested —
# see test-workflow-tool-deps.sh.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

if [ "$#" -eq 0 ]; then
    set -- ".github/workflows"
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "::error::python3 not found — the workflow tool check could not run"
    exit 1
fi

if ! python3 -c "import yaml" 2>/dev/null; then
    echo "::error::PyYAML not available — the workflow tool check could not run"
    echo "Install python3-yaml; without it the workflow files cannot be parsed and"
    echo "every job in them would go unexamined."
    exit 1
fi

files=()
for target in "$@"; do
    if [ -d "$target" ]; then
        # maxdepth 1 because GitHub only reads workflows directly in this
        # directory; a nested .yml is not a workflow.
        found="$(find "$target" -maxdepth 1 -type f \( -name '*.yml' -o -name '*.yaml' \) | sort)"
        if [ -z "$found" ]; then
            echo "::error::no workflow files in '$target' — the workflow tool check could not run"
            exit 1
        fi
        while IFS= read -r one; do
            [ -n "$one" ] && files+=("$one")
        done <<<"$found"
    elif [ -f "$target" ]; then
        files+=("$target")
    else
        echo "::error::'$target' does not exist — the workflow tool check could not run"
        exit 1
    fi
done

REPO_ROOT="$REPO_ROOT" python3 - "${files[@]}" <<'PYTHON'
import os
import re
import sys

import yaml

# tool -> (apt package, pattern matching a use of the tool in shell text)
#
# The word boundaries are spelled out rather than using \b because `.`, `/` and
# `-` are word characters to a shell reader and are not to \b: without this,
# `rg` matches inside `target/rg-out` and inside `cargo-rg`.
EDGE = r"(?:^|[^\w./-])%s(?![\w./-])"
TOOLS = {
    "rg": ("ripgrep", re.compile(EDGE % "rg", re.M)),
}

REPO_ROOT = os.environ["REPO_ROOT"]
SCRIPT_REFERENCE = re.compile(r"ci/scripts/[\w.-]+\.sh")

script_cache = {}


def tools_used(text, seen):
    """Packages `text` needs, following the repo scripts it invokes."""
    needed = {package for _, (package, pattern) in TOOLS.items() if pattern.search(text)}
    for reference in SCRIPT_REFERENCE.findall(text):
        if reference in seen:
            continue
        seen.add(reference)
        if reference not in script_cache:
            path = os.path.join(REPO_ROOT, reference)
            try:
                with open(path, encoding="utf-8") as handle:
                    script_cache[reference] = handle.read()
            except OSError:
                # Not every match is an invocation — this pattern finds script
                # paths inside heredocs and test fixtures too, and the self-test
                # for this gate contains several that do not exist. A workflow
                # naming a script that really is missing is a different fault,
                # and one the job reports loudly at `bash:` the first time it
                # runs, so passing over it here hides nothing.
                script_cache[reference] = ""
        needed |= tools_used(script_cache[reference], seen)
    return needed


def installed_packages(text):
    """Packages an `apt-get install` line in `text` names."""
    packages = set()
    # A trailing backslash continues the command onto the next line, and two of
    # these workflows already write their package lists that way. Reading the
    # lines separately drops every package after the first line — a false red on
    # a job that installs exactly what it should.
    for line in text.replace("\\\n", " ").splitlines():
        if "apt-get install" not in line:
            continue
        # Everything after the subcommand; options are dropped, and so is the
        # `-y` that every one of these lines carries.
        words = line.split("apt-get install", 1)[1].split()
        packages.update(word for word in words if not word.startswith("-"))
    return packages


def steps_of(job):
    steps = job.get("steps")
    return steps if isinstance(steps, list) else []


failures = 0
jobs_checked = 0

for path in sys.argv[1:]:
    with open(path, encoding="utf-8") as handle:
        # A workflow's `on:` key is parsed by YAML 1.1 as the boolean True.
        # Irrelevant here — only `jobs` is read — but it is the reason this
        # does not assert on the document's shape.
        document = yaml.safe_load(handle)

    jobs = (document or {}).get("jobs")
    if not isinstance(jobs, dict):
        continue

    for job_name, job in jobs.items():
        if not isinstance(job, dict):
            continue
        jobs_checked += 1

        run_texts = [
            step["run"]
            for step in steps_of(job)
            if isinstance(step, dict) and isinstance(step.get("run"), str)
        ]

        needed = set()
        for text in run_texts:
            needed |= tools_used(text, set())

        installed = set()
        for text in run_texts:
            installed |= installed_packages(text)

        for package in sorted(needed - installed):
            print(
                f"::error::{path}: job '{job_name}' runs a script that needs "
                f"{package}, which it never installs"
            )
            failures += 1

if jobs_checked == 0:
    print("::error::no jobs found in the paths given — the workflow tool check found nothing to check")
    sys.exit(1)

if failures:
    print()
    print(f"{failures} job(s) above run a tool they do not install. The runner image")
    print("does not ship it, so the step dies with 'command not found'. Compare against")
    print("a job that runs the same script and passes — the install line is already there.")
    sys.exit(1)

print(f"All {jobs_checked} workflow job(s) install the tools they use ({len(TOOLS)} tool(s) checked).")
PYTHON
