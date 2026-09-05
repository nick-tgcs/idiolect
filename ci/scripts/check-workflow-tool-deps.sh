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
# LIMITATION: a command carried by a VARIABLE — `S='rg …'; $S`, or
# `bash -c "$S"` — is not followed. The scanner tracks assignments through
# conditionals, subshells, functions and namerefs to answer the same question
# about apt, and a second, weaker copy of that here would be a second answer to
# it. The direction is a use unseen rather than one invented, and it is not the
# only line of defence: every script that uses ripgrep now refuses to start
# without it, so a job that reaches one this way fails loudly on its first run
# instead of passing silently, which is the defect this gate exists for.
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

# Options that introduce a command rather than a file — `find … -exec rg … +`.
RUNS_WHAT_FOLLOWS = {"-exec", "-execdir", "-ok", "-okdir"}

# What terminates one of those actions, after which find's own expression
# continues.
FIND_ACTION_ENDS = {";", "+"}

# The body of a backtick substitution, which shlex does not tokenise as one.
TICKED = re.compile(r"`([^`]*)`")

# A heredoc opener, with the quoting of its delimiter — the thing that decides
# whether the body is expanded or passed on as it was written.
HEREDOC_OPENER = re.compile(r"""<<-?\s*(['"]?)[A-Za-z_][\w-]*\1""")

# The openers of a process substitution, `<( … )` and `>( … )`.
PROCESS_SUBSTITUTION = re.compile(r"[<>]\(")

# Stands in for a `$` while process substitutions are being read. A NUL cannot
# appear in a shell word — bash drops it — so nothing real can collide with it.
MARKER = "\0"


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
    """Every command in a block of shell, with the block it was written in.

    The block comes along because quoting is a property of what was WRITTEN:
    shlex strips the quotes, and one question below can only be answered by
    looking at the line again.
    """
    expanding = False
    for _, block, in_heredoc in shell.shell_commands(text):
        if in_heredoc:
            # A heredoc body is data the shell runs none of — unless its
            # delimiter was UNQUOTED, in which case bash expands the data
            # first, and a substitution written there runs (Codex, on
            # d3039b5). The delimiter decides, and it is on the opening line
            # this loop has already been past.
            if expanding:
                for body in shell.substitution_bodies(block) + TICKED.findall(block):
                    yield from commands_of(body)
            continue

        # `<<EOF` expands, `<<'EOF'` and `<<"EOF"` do not. Read from the
        # opener rather than from the scanner's delimiter, which keeps the
        # NAME and drops the quotes that are the whole question here.
        expanding = any(
            not opener.group(1) for opener in HEREDOC_OPENER.finditer(block)
        )
        masked, _ = shell.mask_expressions(block)
        try:
            words = shell.lex_words(masked)
        except ValueError:
            # An unfinished quote. The apt scanner announces these; here the
            # cost of passing over one is a tool use unseen, which the script
            # itself now reports the moment it runs without its tool.
            continue
        for command in commands_in(words):
            yield command, block


# Words this run could not decide about, announced once at the end rather than
# resolved. A set because the same line reaches here once per job that runs it.
undecided = set()


def analysed_command(words, block=""):
    """(packages this one command runs, the repo scripts it runs)."""
    packages = set()
    references = set()

    # A substitution is shell of its own, in either spelling. `$( … )` comes
    # from the scanner, which reads it out of a single quoted token as well as
    # a split one; the backtick form is taken from the rejoined command,
    # because shlex gives backticks no meaning and leaves the opening one
    # welded to the name in front of it — `hits=`rg …`` lexes with `rg` inside
    # the first word, where nothing looks for a command.
    inner = []
    for word in words:
        if word.literal_dollar:
            # Single-quoted: `echo '$(rg …)'` runs nothing, and the lexer
            # already carries that. Discarding it read a job's own
            # documentation as a dependency (Codex, on f9efff8).
            #
            # ...but the flag describes the WORD, and a word can be part
            # literal and part live: `'$literal'"$(rg …)"` runs its second
            # half, and by the time quoting is stripped both dollars look the
            # same (Codex, on a6bf836). Neither answer is safe, so this one is
            # not given — announced instead, the way the apt scanner announces
            # what it cannot resolve, because a skip nobody can see is a skip
            # nobody can audit.
            #
            # Whether it is one or the other is asked of the LINE: a word that
            # appears in it wrapped in single quotes, whole, is the literal
            # case and decided. Anything else is announced. Being wrong about
            # this costs a notice, never a verdict.
            if shell.substitution_bodies(word.value) and f"'{word.value}'" not in block:
                undecided.add(word.value)
            continue
        inner += shell.substitution_bodies(word.value)

    # The rest is asked of the REJOINED command rather than of single words,
    # because shlex gives neither construct any meaning: `<(` arrives as `<`
    # and `(`, and a backtick stays welded to the name in front of it. The
    # words are rejoined the way they were written, so `< (` — a redirection
    # and a subshell — cannot become a process substitution on the way.
    #
    # A word holding a QUOTED `<(` is left out of it: bash performs no process
    # substitution inside quotes of either kind, so `echo '<(rg …)'` prints
    # text. Rewriting it to `$( … )` undid, for this construct, the very check
    # the same commit added for `$( … )` itself (Codex, on 5fd4cef).
    #
    # Only a word holding the OPENER, though. Dropping every quoted word threw
    # away the command name in `< <("rg" …)`, where the quotes are around a
    # word INSIDE a substitution that is itself unquoted and runs (Codex, on
    # dcf9333). Which is the same distinction as the line above, one level in:
    # quoting an argument is not quoting the construct.
    #
    # And the opener may be a word of its OWN: `<"("` redirects from a file
    # named `(`, so rejoining without the quoting produced a `<(` nobody wrote
    # (Codex, on e9866ba). Any quoted word holding a bracket is left out,
    # rather than only one already spelled `<(`.
    rendered = "".join(
        (" " if word.space_before else "") + word.value
        for word in words
        if not (word.quoted and "(" in word.value)
    )
    # `<( … )` and `>( … )` run their contents in a process of their own, and
    # the command word in front of them is something else entirely. Spelled as
    # `$( … )` for the scanner's reader rather than balanced again here: the
    # brackets, quotes and escapes are the same, and a second implementation of
    # them is a second answer.
    #
    # `$(` is hidden first, so what comes back is only what the rewrite put
    # there. Without that, the rendered line — which now keeps quoted words,
    # for the reason above — hands its own `'$(rg …)'` straight to a reader
    # that cannot tell it was quoted. The bracket stays where it is, so
    # nothing's balance changes, and the marker is put back in whatever is
    # extracted in case a process substitution holds a command substitution.
    # An unquoted `$( … )` splits into `$` and `(` too, so it is read from the
    # same rejoined line — from the UNQUOTED words only, since a quoted one is
    # the per-word reader's business above, where the literal case is settled.
    inner += shell.substitution_bodies(
        "".join(
            (" " if word.space_before else "") + word.value
            for word in words
            if not word.quoted
        )
    )

    hidden = rendered.replace("$(", MARKER + "(")
    inner += [
        body.replace(MARKER + "(", "$(")
        for body in shell.substitution_bodies(PROCESS_SUBSTITUTION.sub("$(", hidden))
    ]
    inner += TICKED.findall(
        "".join(
            (" " if word.space_before else "") + word.value
            for word in words
            if not word.literal_backtick
        )
    )
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
            found, referenced = analysed_command(command, block)
            packages |= found
            references |= referenced
        return packages, references

    word = shell.command_word(words)
    if word is None:
        return packages, references

    at = next(
        (position for position, other in enumerate(words) if other is word),
        len(words),
    )

    # `xargs rg …`, `timeout 30 rg …` and `timeout 30 nice rg …`: a wrapper
    # runs its argument, and that argument may be another wrapper (Codex, on
    # f9efff8). Which word the command is cannot be known without a table of
    # every option's arity — the scanner declines to hold one for `sudo -u`,
    # and this declines for the same reason — so the first argument that is
    # neither an option nor a bare number is taken. Being wrong here leaves a
    # use unseen; it never invents one.
    while words[at].value.rsplit("/", 1)[-1] in WRAPPERS:
        following = next(
            (
                position
                for position in range(at + 1, len(words))
                if not words[position].value.startswith("-")
                and not words[position].value.isdigit()
            ),
            None,
        )
        if following is None:
            break
        at = following

    # `find crates -exec rg -n TODO {} +` runs rg once per match. FIND's
    # option, and only find's: to `echo` a `-exec` is an argument like any
    # other, and giving it this meaning everywhere demanded ripgrep of a job
    # that prints the word (Codex, on 5fd4cef).
    #
    # Three things about an action, all of them found the hard way (Codex, on
    # a6bf836). Quoting it changes nothing, because the quotes are the SHELL's
    # and find is handed `-exec` either way. An action is a COMMAND, so
    # everything known about commands — wrappers, shells, tools — applies
    # inside it, which is why it is read by recursion rather than by moving a
    # cursor. And `-and` is implicit between adjacent expressions, so a find
    # may hold SEVERAL, of which the first is not the interesting one.
    if words[at].value.rsplit("/", 1)[-1] == "find":
        position = at + 1
        while position < len(words):
            if words[position].value not in RUNS_WHAT_FOLLOWS:
                position += 1
                continue
            action = position + 1
            end = action
            while end < len(words) and words[end].value not in FIND_ACTION_ENDS:
                end += 1
            found, referenced = analysed_command(words[action:end], block)
            packages |= found
            references |= referenced
            position = end + 1

    # Asked HERE rather than of the whole command, because a wrapper hides the
    # shell behind it: with `timeout 30 bash -c 'rg …'` the command is still
    # `timeout` at the top of this function, so nothing looked inside the
    # program bash was handed (Codex, on 5fd4cef).
    if shell.runs_a_script(words[at:]):
        # The program is ONE quoted word that nothing here would otherwise look
        # inside — the shell is the command and the tool is a character in a
        # string. The scanner reads these because a script handed over this way
        # can install packages; it is read here for the same reason.
        script = shell.script_argument(words[at:])
        if script is not None:
            found, referenced = analysed(script.value)
            return packages | found, references | referenced

    command = words[at]
    name = command.value.rsplit("/", 1)[-1]
    package = TOOLS.get(name)
    if package is not None:
        packages.add(package)

    if SCRIPT_REFERENCE.fullmatch(command.value):
        # An executable script run by its own path.
        references.add(command.value)
    elif name in shell.SHELL_COMMANDS or name in SOURCING:
        # `bash ci/scripts/x.sh`: the script is an ARGUMENT, and only of a
        # command that runs one. Taking the path from any word at all followed
        # what a `coverage_gate="ci/scripts/test-all.sh"` merely NAMES, and
        # through it every script in the suite.
        #
        # ONE operand, the first: `bash driver.sh other.sh` runs driver.sh and
        # hands the second path to it as `$1`, so following both rejected a
        # driver for what its argument does.
        operand = at + 1
        while operand < len(words) and words[operand].value.startswith("-"):
            if words[operand].value in shell.SHELL_OPTIONS_WITH_ARGUMENT:
                operand += 1
            operand += 1
        if operand < len(words) and SCRIPT_REFERENCE.fullmatch(words[operand].value):
            references.add(words[operand].value)
    return packages, references


def analysed(text):
    """(packages this shell text RUNS, the repo scripts it RUNS)."""
    packages = set()
    references = set()
    if not any(candidate.search(text) for candidate in CANDIDATES) and not SCRIPT_REFERENCE.search(text):
        return packages, references

    for words, block in commands_of(text):
        found, referenced = analysed_command(words, block)
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
        for index, (_, job_name, job, document) in enumerate(jobs):
            path = os.path.join(work, f"{index}.yml")
            # The whole workflow, with every OTHER job removed. Keeping only
            # the `run:` strings left the scanner unable to resolve a
            # `${{ env.INSTALL }}` — the value it names lives in the document
            # around the step — so a job was failed for an install it makes
            # (Codex, on f9efff8). What has to go is the sibling jobs, whose
            # packages are not this job's.
            alone = {key: value for key, value in document.items() if key != "jobs"}
            alone["jobs"] = {job_name: job}
            with open(path, "w", encoding="utf-8") as handle:
                yaml.safe_dump(alone, handle)
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
        jobs_found.append((path, job_name, job, document))

jobs_checked = len(jobs_found)
failures = 0
installed_by_job = installed_packages(jobs_found)

for index, (path, job_name, job, _) in enumerate(jobs_found):
    needed = set()
    for step in steps_of(job):
        if isinstance(step, dict) and isinstance(step.get("run"), str):
            needed |= tools_used(step["run"])

    for package in sorted(needed - installed_by_job[index]):
        print(
            f"::error::{path}: job '{job_name}' runs a script that needs "
            f"{package}, which it never installs"
        )
        failures += 1

for word in sorted(undecided):
    print(
        f"notice: cannot tell which substitution in {word!r} is literal and "
        "which runs — not checked"
    )

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
