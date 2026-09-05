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

# command name -> apt package that provides it
TOOLS = {
    "rg": "ripgrep",
}

# A cheap over-approximating filter, asked before the lexer is. Lexing costs
# real time on a large file — this repository has a 6,000-line self-test, which
# takes minutes — and text holding neither a tool name nor a script path cannot
# contribute either. The regex decides only whether to LOOK; the lexer decides
# every verdict, so a match here that turns out to be a word inside an echo
# costs a parse and changes nothing.
#
# The word boundaries are spelled out rather than using \b because `.` and `-`
# are word characters to a shell reader and are not to \b: without this, `rg`
# matches inside `target/rg-out` and inside `cargo-rg`.
#
# A `/` before the name is NOT a boundary to exclude: `/usr/bin/rg` runs
# ripgrep, and the analysis behind this filter strips directories exactly as
# the scanner does for `/usr/bin/apt-get`. Excluding it made the filter
# narrower than the thing it filters for, so the block was never even looked at
# (Codex, on 03da267).
EDGE = r"(?:^|[^\w.-])%s(?![\w./-])"
CANDIDATES = [re.compile(EDGE % re.escape(name), re.M) for name in TOOLS]

REPO_ROOT = os.environ["REPO_ROOT"]
SCANNER = os.environ["SCANNER"]

# The apt scanner as a MODULE, for the question this gate asks of shell text:
# which words are commands. It already answers that — it has to, to tell an
# `apt-get` from the word `apt-get` inside an echo — and it models heredoc
# bodies, quoting, continuations, pipelines and invocation prefixes to do it.
# A second reader here would be a second answer: reading raw text called
# `echo "use rg to search"` a use of rg and demanded the package for it
# (Codex, on d6df5fd), and this suite's own fixtures did the same.
sys.path.insert(0, os.path.dirname(os.path.abspath(SCANNER)))
try:
    import workflow_apt_deps as shell  # noqa: E402
except BaseException as error:  # SystemExit included: a scanner that exits on import
    # A scanner that cannot be loaded must not read as a repository with no
    # tool uses in it, which is what an empty analysis would look like.
    print("::error::the apt scanner failed — the workflow tool check could not run")
    print(error)
    sys.exit(1)
SCRIPT_REFERENCE = re.compile(r"(?:\./)?ci/scripts/[\w.-]+\.sh")

# `source x` and `. x` run a script in the current shell rather than a new one.
SOURCING = {"source", "."}

# Commands whose ARGUMENT is the command that runs.
WRAPPERS = {"xargs", "timeout", "nice", "ionice", "stdbuf", "nohup", "setsid"}

# The body of a backtick substitution, which shlex does not tokenise as one.
TICKED = re.compile(r"`([^`]*)`")


script_cache = {}


def commands_in(words):
    """Every command among these already-lexed words.

    A command, not a segment: `printf … | rg …` is two of them, and the second
    is how test-real-adapter-deps.sh calls rg.
    """
    index = 0
    while index <= len(words):
        stop = shell.command_ends(words, index)
        if stop > index:
            yield words[index:stop]
        index = stop + 1


def commands_of(text):
    """Every command in a block of shell, heredoc bodies excluded."""
    for _, block, in_heredoc in shell.shell_commands(text):
        if in_heredoc:
            continue
        masked, _ = shell.mask_expressions(block)
        try:
            words = shell.lex_words(masked)
        except ValueError:
            # An unfinished quote. The apt scanner announces these; here the
            # cost of passing over one is a tool use unseen, which the script
            # itself now reports the moment it runs without its tool.
            continue
        yield from commands_in(words)


def analysed_command(words):
    """(packages this one command runs, the repo scripts it runs)."""
    packages = set()
    references = set()

    # A substitution is shell of its own, in either spelling. `$( … )` comes
    # from the scanner, which reads it out of a single quoted token as well as
    # a split one; the backtick form is taken from the rejoined command,
    # because shlex gives backticks no meaning and leaves the opening one
    # welded to the name in front of it — `hits=`rg …`` lexes with `rg` inside
    # the first word, where nothing looks for a command.
    inner = [body for word in words for body in shell.substitution_bodies(word.value)]
    inner += TICKED.findall(" ".join(word.value for word in words))
    for body in inner:
        found, referenced = analysed(body)
        packages |= found
        references |= referenced

    # Declaring a function runs none of it, but the body is shell that runs
    # WHERE IT IS CALLED, and a workflow that declares and calls one in the
    # same block runs it here. Read rather than tracked: over-approximating a
    # call costs an install this gate would have asked for anyway, while
    # missing one is a job that dies on a tool nobody said it needed.
    definition = shell.defined_function(words, 0) if words else None
    if definition is not None:
        for command in commands_in(definition[1]):
            found, referenced = analysed_command(command)
            packages |= found
            references |= referenced
        return packages, references

    if shell.runs_a_script(words):
        # `bash -c 'rg …'` runs rg, and the program is ONE quoted word that
        # nothing here would otherwise look inside — the shell is the command
        # and the tool is a character in a string (Codex, on 03da267). The
        # scanner reads these because a script handed over this way can install
        # packages; it is read here for the same reason.
        script = shell.script_argument(words)
        if script is not None:
            found, referenced = analysed(script.value)
            return packages | found, references | referenced

    word = shell.command_word(words)
    if word is None:
        return packages, references

    named = [word]
    if word.value.rsplit("/", 1)[-1] in WRAPPERS:
        # `xargs rg …` and `timeout 30 rg …` run their argument. Which word
        # that is cannot be known without a table of every option's arity —
        # the scanner declines to hold one for `sudo -u`, and this declines
        # for the same reason — so the first argument that is neither an
        # option nor a bare number is taken. Being wrong here leaves a use
        # unseen; it never invents one.
        for argument in words[words.index(word) + 1:]:
            if argument.value.startswith("-") or argument.value.isdigit():
                continue
            named.append(argument)
            break

    for candidate in named:
        name = candidate.value.rsplit("/", 1)[-1]
        package = TOOLS.get(name)
        if package is not None:
            packages.add(package)
        if SCRIPT_REFERENCE.fullmatch(candidate.value):
            # An executable script run by its own path.
            references.add(candidate.value)
        elif name in shell.SHELL_COMMANDS or name in SOURCING:
            # `bash ci/scripts/x.sh`: the script is an ARGUMENT, and only of a
            # command that runs one. Taking the path from any word at all
            # followed what a `coverage_gate="ci/scripts/test-all.sh"` merely
            # NAMES, and through it every script in the suite.
            references.update(
                argument.value
                for argument in words[words.index(candidate) + 1:]
                if SCRIPT_REFERENCE.fullmatch(argument.value)
            )
    return packages, references


def analysed(text):
    """(packages this shell text RUNS, the repo scripts it RUNS)."""
    packages = set()
    references = set()
    if not any(candidate.search(text) for candidate in CANDIDATES) and not SCRIPT_REFERENCE.search(text):
        return packages, references

    for words in commands_of(text):
        found, referenced = analysed_command(words)
        packages |= found
        references |= referenced
    return packages, references


def script_analysis(reference):
    """`analysed` for a repo script, read once however many jobs reach it."""
    reference = reference.removeprefix("./")
    if reference not in script_cache:
        path = os.path.join(REPO_ROOT, reference)
        try:
            with open(path, encoding="utf-8") as handle:
                body = handle.read()
        except OSError:
            # A workflow naming a script that is not there is a different fault,
            # and one the job reports loudly at `bash:` the first time it runs.
            body = ""
        script_cache[reference] = analysed(body)
    return script_cache[reference]


def tools_used(text):
    """Packages `text` needs, following the repo scripts it runs."""
    needed, queue = analysed(text)
    seen = set()
    while queue:
        reference = queue.pop().removeprefix("./")
        if reference in seen:
            continue
        seen.add(reference)
        packages, references = script_analysis(reference)
        needed |= packages
        queue |= references - seen
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
        try:
            # A workflow's `on:` key is parsed by YAML 1.1 as the boolean True.
            # Irrelevant here — only `jobs` is read — but it is the reason this
            # does not assert on the document's shape.
            document = yaml.safe_load(handle)
        except yaml.YAMLError as error:
            print(f"::error::{path} is not valid YAML ({error})")
            sys.exit(1)

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
        needed |= tools_used(text)

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
