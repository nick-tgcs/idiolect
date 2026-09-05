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

SCANNER="$SCRIPT_DIR/workflow_apt_deps.py"
if [ ! -f "$SCANNER" ]; then
    echo "::error::$SCANNER is missing — the workflow tool check could not run"
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

REPO_ROOT="$REPO_ROOT" SCANNER="$SCANNER" python3 - "${files[@]}" <<'PYTHON'
import os
import re
import subprocess
import sys
import tempfile

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
SCANNER = os.environ["SCANNER"]
SCRIPT_REFERENCE = re.compile(r"ci/scripts/[\w.-]+\.sh")


def active(text):
    """`text` without its comment lines.

    A `#` line is not a command, in a `run:` block or in a script. Without this
    the guard comment added to each rg-using script — which quotes `if rg ...`
    to explain itself — reads as a use of the tool, and so does an install
    someone commented out. Only whole-line comments: a `#` mid-line may be
    inside quotes, a URL fragment or a parameter expansion, and deciding which
    is shell lexing, which is what the apt scanner below is for.
    """
    return "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith("#")
    )

script_cache = {}


def tools_used(text, seen):
    """Packages `text` needs, following the repo scripts it invokes."""
    text = active(text)
    needed = {package for package, pattern in TOOLS.values() if pattern.search(text)}
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


def installed_packages(jobs):
    """Packages each job installs, keyed by its index in `jobs`.

    Delegated to workflow_apt_deps.py rather than scanned here. Searching raw
    text for `apt-get install` credited a job with a package it had COMMENTED
    OUT, and with one merely echoed or quoted in a heredoc — a false negative
    in the gate written to stop false negatives (Codex, on a5517fa). Knowing
    which words are a command and which are text is shell lexing, and this
    repository already has a lexer for exactly this question, with 420 cases
    behind it.

    Each job is handed over as a workflow of its own so the records come back
    attributable to it; the packages of a sibling job are not this job's.
    """
    installed = {index: set() for index in range(len(jobs))}
    with tempfile.TemporaryDirectory() as work:
        paths = []
        for index, (_, job_name, run_texts) in enumerate(jobs):
            path = os.path.join(work, f"{index}.yml")
            document = {
                "jobs": {job_name: {"steps": [{"run": text} for text in run_texts]}}
            }
            with open(path, "w", encoding="utf-8") as handle:
                yaml.safe_dump(document, handle)
            paths.append(path)

        if not paths:
            return installed

        result = subprocess.run(
            [sys.executable, SCANNER, *paths],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            # A scanner that could not run must not read as a job that installs
            # nothing — that would report every job on the list as broken.
            print("::error::the apt scanner failed — the workflow tool check could not run")
            print(result.stderr.strip())
            sys.exit(1)

        for line in result.stdout.splitlines():
            fields = line.split("\t")
            if len(fields) != 4 or fields[0] != "PKG":
                continue
            index = int(os.path.basename(fields[1]).removesuffix(".yml"))
            installed[index].add(fields[3])
    return installed


def steps_of(job):
    steps = job.get("steps")
    return steps if isinstance(steps, list) else []


jobs_found = []

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
        run_texts = [
            step["run"]
            for step in steps_of(job)
            if isinstance(step, dict) and isinstance(step.get("run"), str)
        ]
        jobs_found.append((path, job_name, run_texts))

jobs_checked = len(jobs_found)
failures = 0
installed_by_job = installed_packages(jobs_found)

for index, (path, job_name, run_texts) in enumerate(jobs_found):
    needed = set()
    for text in run_texts:
        needed |= tools_used(text, set())

    for package in sorted(needed - installed_by_job[index]):
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
