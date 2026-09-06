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
# LIMITATION: a job's NEEDS are followed through the scripts it runs; its
# INSTALLS are not. A script that installs its own tool is therefore reported
# as a job that forgot it (Codex, on 6a4cc47) — a false red, which is the loud
# direction and is fixed by naming the package in the workflow, where the other
# 38 jobs already name theirs.
#
# It stays that way because the honest fix costs more than it buys, which was
# measured rather than assumed: handing a script to the apt scanner takes over
# thirty seconds for this repository's own self-test, and the cheap
# alternative — pulling out the lines that look like installs — loses the very
# context that makes the answer right, since a fixture install inside a heredoc
# would then be credited as a real one. No script here installs anything, and a
# false GREEN is the failure this gate exists to prevent.
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
# Overridable so the self-test can point it at a tree of fixture scripts: what
# a job installs may be installed by a SCRIPT it runs, and testing that needs
# a script this repository does not have.
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/../.." && pwd)}"

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
# A repository script, however it is spelled: bare, `./`-prefixed, absolute, or
# behind `${{ github.workspace }}` — which masking leaves as a prefix on the
# word (Codex, on 951f419). The prefix must END at a slash, so a `notci/scripts`
# is a different directory and not this one.
SCRIPT_REFERENCE = re.compile(r"(?:\S*/)?(ci/scripts/[\w.-]+\.sh)")

# `source x` and `. x` run a script in the current shell rather than a new one.
SOURCING = {"source", "."}

# Options that make a command print something and exit, whatever follows.
# Probed, every one: `--show-limits` and `--usage` were in here on the strength
# of their names alone, and xargs RUNS the command after the first while it has
# never heard of the second (Codex, on e9ab98e). The short spellings belong to
# util-linux — `ionice -h rg` prints usage, `setsid -V rg` prints a version —
# and are kept per wrapper, since `-V` means something else elsewhere.
TERMINAL_OPTIONS = frozenset({"--help", "--version"})
WRAPPER_TERMINAL_LETTERS = {
    "ionice": "hV",
    "setsid": "hV",
}

# Every long option each wrapper has, so an unambiguous ABBREVIATION can be
# resolved to it: GNU accepts `xargs --max-a 1 rg …` and consumes the `1`
# (Codex, on e9ab98e). Ambiguous prefixes resolve to nothing, which is what
# getopt does with them.
WRAPPER_LONG_OPTIONS = {
    "xargs": (
        "--null", "--arg-file", "--delimiter", "--eof", "--replace",
        "--max-lines", "--max-args", "--open-tty", "--max-procs",
        "--interactive", "--process-slot-var", "--no-run-if-empty",
        "--max-chars", "--show-limits", "--verbose", "--exit", "--help",
        "--version",
    ),
    "timeout": (
        "--preserve-status", "--foreground", "--kill-after", "--signal",
        "--verbose", "--help", "--version",
    ),
    "nice": ("--adjustment", "--help", "--version"),
    "ionice": (
        "--class", "--classdata", "--pid", "--pgid", "--uid", "--ignore",
        "--help", "--version",
    ),
    "stdbuf": ("--input", "--output", "--error", "--help", "--version"),
    "nohup": ("--help", "--version"),
    "setsid": ("--ctty", "--fork", "--wait", "--help", "--version"),
}



def resolved(token, wrapper):
    """A long option's full spelling, if `token` abbreviates exactly one."""
    known = WRAPPER_LONG_OPTIONS.get(wrapper, ())
    if token in known:
        return token
    matches = [option for option in known if option.startswith(token)]
    return matches[0] if len(matches) == 1 else token

# Commands whose ARGUMENT is the command that runs.
WRAPPERS = {"xargs", "timeout", "nice", "ionice", "stdbuf", "nohup", "setsid"}

# Their own options that take a value, and how many bare operands of their own
# come before the command. Held per wrapper rather than guessed: the scanner
# declines to hold such a table for `sudo -u` and says so, but there the cost
# was a value unread, while here it is the command itself.
# Their short option LETTERS that take a value, for the clustered spelling:
# `xargs -rn 1 rg …` is the same as `xargs -r -n 1 rg …`, and matching whole
# tokens skipped `-rn` without consuming the `1` (Codex, on c1c9438).
WRAPPER_LETTERS_WITH_ARGUMENT = {
    # `l`, `i` and `e` are absent deliberately: GNU spells them
    # `-l[MAX-LINES]`, `-i[REPLACE]` and `-e[EOF]`, so a value must be ATTACHED
    # and a bare one takes nothing — `xargs -l rg …` runs rg, while
    # `xargs -L rg …` swallows it and reports `invalid number "rg"` (Codex, on
    # 432effa; both probed). Leaving them out is the whole rule: an attached
    # value is part of the same token, so nothing is consumed either way, and a
    # second table for "optional" would only be able to disagree with this one.
    "xargs": "InPLsadE",
    "timeout": "sk",
    "nice": "n",
    "ionice": "cnpPu",
    "stdbuf": "ioe",
}

WRAPPER_OPTIONS_WITH_ARGUMENT = {
    # `--replace[=R]` and `--eof[=END]` take their value ATTACHED, so a bare
    # one consumes nothing and the word after it is the command (Codex, on
    # 951f419). Same for their short spellings `-i` and `-e`.
    # The long spellings come from each tool's own `--help`, in full rather
    # than as they were needed: `--max-chars` was missing and swallowed the
    # command (Codex, on 631fa9f), and a table copied in part is a table that
    # will be wrong again. `--eof` and `--replace` are absent for the reason
    # their short forms are: their value is optional and must be attached.
    "xargs": frozenset({
        "-I", "-n", "-P", "-L", "-s", "-a", "-d", "-E",
        "--max-args", "--max-procs", "--max-lines", "--max-chars",
        "--arg-file", "--delimiter", "--process-slot-var",
    }),
    "timeout": frozenset({"-s", "--signal", "-k", "--kill-after"}),
    "nice": frozenset({"-n", "--adjustment"}),
    "ionice": frozenset({"-c", "-n", "-p", "-P", "-u", "--class", "--classdata",
                         "--pid", "--pgid", "--uid"}),
    "stdbuf": frozenset({"-i", "-o", "-e", "--input", "--output", "--error"}),
}

# `timeout DURATION COMMAND …` — the duration is timeout's, and it may carry a
# suffix, so it is not recognised by looking like a number.
WRAPPER_OPERANDS = {"timeout": 1}

# Every wrapper needs an entry, and this is the third round in which a table
# filled in one entry at a time was wrong (Codex, on 707d953: `setsid` and
# `nohup` were wrappers with no options listed, so their abbreviations resolved
# to nothing and the command after one was read as the wrapped command). Making
# the omission impossible is cheaper than noticing it again.
_unlisted = sorted(set(WRAPPERS) - set(WRAPPER_LONG_OPTIONS))
if _unlisted:
    print(
        f"::error::wrapper(s) with no option list: {', '.join(_unlisted)}"
        " — the workflow tool check could not run"
    )
    sys.exit(1)

# Options that introduce a command rather than a file — `find … -exec rg … +`.
RUNS_WHAT_FOLLOWS = {"-exec", "-execdir", "-ok", "-okdir"}

# What terminates one of those actions, after which find's own expression
# continues.
FIND_ACTION_ENDS = {";", "+"}

# The body of a backtick substitution, which shlex does not tokenise as one.
# The opener cannot be an ESCAPED backtick: in ``echo \`literal `rg …` `` the
# first one is data and the live pair after it runs, while starting there
# paired it with the live opener and left the real closer unmatched (Codex, on
# 631fa9f).
#
# A backslash escapes a backtick, and that is how the legacy form NESTS:
# `` `echo \`rg …\`` `` is one substitution holding another, so the outer
# pair must not close on the inner opener (Codex, on d32dd1d).
TICKED = re.compile(r"(?<!\\)`((?:[^`\\]|\\.)*)`", re.S)


def ticked(text):
    """The body of each backtick substitution in `text`, one level down.

    The escapes come off what is returned: inside a substitution bash reads
    `\\`` as the delimiter of a nested one, so the body is what the shell would
    see, and the same reader finds the next level in it.
    """
    return [found.replace("\\`", "`") for found in TICKED.findall(text)]

# A heredoc opener, with the quoting of its delimiter — the thing that decides
# whether the body is expanded or passed on as it was written.
# The delimiter is a WORD — `<<123` is legal, and `<<'123'` is the literal
# form of it (Codex, on 951f419) — so it is anything up to whitespace or the
# operators that would end it.
HEREDOC_OPENER = re.compile(r"""<<-?\s*([^\s;&|<>()]+)""")

# What a shell removes on its way to a command name.
QUOTING_CHARACTERS = re.compile(r"""[\\'"]""")

# An ESCAPED backslash. Replacing each pair before looking for a backtick is
# what makes the escape test count parity: `\\`rg …`` opens a substitution,
# because the two backslashes are one literal backslash and the tick after them
# is live (Codex, on 92bf47b). Replaced rather than removed, so nothing that
# was apart closes up.
PAIRED = re.compile(r"\\\\")

# A backslash and the character it quotes, anywhere.
ESCAPED = re.compile(r"\\.", re.S)

# The pairs a HEREDOC body reads: bash gives a backslash meaning there only
# before `$`, a backtick, another backslash, or a newline.
HEREDOC_ESCAPED = re.compile(r"\\[$`\\]")

# A line continuation: the one escaped pair bash removes outright.
CONTINUED = re.compile(r"\\\n")

# What makes a delimiter quoted, and so its body literal: not only a pair
# around the whole word. `<<\EOF` and `<<E"OF"` are both quoted to bash, which
# strips the quoting and stops expanding the body (Codex, on d32dd1d).
QUOTING = ("'", '"', "\\")

# The openers of a process substitution, `<( … )` and `>( … )`.
PROCESS_SUBSTITUTION = re.compile(r"[<>]\(")

# Stands in for a `$` while process substitutions are being read. A NUL cannot
# appear in a shell word — bash drops it — so nothing real can collide with it.
MARKER = "\0"


script_cache = {}
script_bodies = {}


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


def expanded(body):
    """The commands an EXPANDING heredoc body runs.

    A body is data the shell runs none of — unless its delimiter was unquoted,
    in which case bash expands the data first, and a substitution written there
    runs (Codex, on d3039b5). Only substitutions: the body is still text being
    written somewhere, and this suite's own fixtures are heredocs full of `rg`
    lines that run nothing.
    """
    # A backslash still quotes in an expanded body: `\$(rg …)` is written out
    # rather than run, and so is `` \` `` (Codex, on 06f621c). Each escaped
    # pair is blanked before anything is read out, two spaces for two
    # characters so the rest of the line stays where it was — and `\\` blanks
    # to nothing that opens anything, leaving a substitution after it live.
    # A backslash-NEWLINE is removed rather than blanked: bash drops the pair
    # before expanding, so a `$` at the end of one line and a `(` at the start
    # of the next are one substitution (Codex, on e08e4a1, and the probe agrees
    # — `$\` + `(echo …)` in an unquoted heredoc runs it).
    #
    # And only the pairs bash actually reads are blanked. In a body a backslash
    # is special before `$`, a backtick and another backslash, and NOWHERE
    # else: `$(r\g …)` keeps its backslash into the substitution, which does
    # its own quote removal and runs rg (Codex, on 631fa9f; probed). Blanking
    # every pair turned that name into `r`.
    joined = HEREDOC_ESCAPED.sub("  ", CONTINUED.sub("", "\n".join(body)))
    for inner in shell.substitution_bodies(joined) + ticked(joined):
        yield from blocks_of(inner)


def blocks_of(text):
    """Every line of shell, with the words it lexes to.

    The line comes along because quoting is a property of what was WRITTEN:
    shlex strips the quotes, and two questions below can only be answered by
    looking at the line again.
    """
    expanding = False
    body = []
    for _, block, in_heredoc in shell.shell_commands(text):
        if in_heredoc:
            # Collected rather than read line by line: a substitution may be
            # written across several of them, and neither the line holding
            # `$(` nor the one holding `)` is a substitution on its own (Codex,
            # on d32dd1d).
            if expanding:
                body.append(block)
            continue
        if body:
            yield from expanded(body)
            body = []
        # `<<EOF` expands, `<<'EOF'` and `<<"EOF"` do not. Read from the
        # opener rather than from the scanner's delimiter, which keeps the
        # NAME and drops the quotes that are the whole question here.
        #
        # One command may open SEVERAL, and each delimiter answers for its own
        # body (Codex, on 4e02d11). Attributing bodies to openers needs the
        # TERMINATOR lines, and those are what the scanner consumes to know a
        # body ended — so where a command's delimiters disagree, this says so
        # instead of applying one of them to everything.
        # ...and where they DISAGREE, the bodies are left alone. Attributing
        # one to its own opener needs the terminator lines, and those are what
        # the scanner consumes to know a body ended. Announcing it was tried
        # first and withdrawn: the notice fired on this repository, because a
        # file of fixtures that quote both spellings lexes as one block, and it
        # printed that whole block. So this is a LIMITATION rather than a
        # notice — the direction is a use unseen in a construct no workflow
        # here writes, and the script that needs the tool still refuses to run
        # without it.
        openers = [
            any(quote in opener.group(1) for quote in QUOTING)
            for opener in HEREDOC_OPENER.finditer(block)
        ]
        expanding = bool(openers) and not any(openers)
        masked, _ = shell.mask_expressions(block)
        try:
            words = shell.lex_words(masked)
        except ValueError:
            # An unfinished quote. The apt scanner announces these; here the
            # cost of passing over one is a tool use unseen, which the script
            # itself now reports the moment it runs without its tool.
            continue
        yield block, words

    if body:
        yield from expanded(body)


# Words this run could not decide about, announced once at the end rather than
# resolved. A set because the same line reaches here once per job that runs it.
undecided = set()


def outside_single_quotes(text):
    """`text` with everything inside single quotes blanked out.

    Offsets are kept, so what is left reads as it was written. Only single
    quotes: they are the one thing that stops a backtick or a `$`, and inside
    them nothing escapes, which is what makes this exact rather than a second
    lexer.
    """
    kept = []
    quote = None
    escaped = False
    for character in text:
        if escaped:
            kept.append(" " if quote == "'" else character)
            escaped = False
            continue
        if character == "\\" and quote != "'":
            kept.append(character)
            escaped = True
            continue
        if quote is None and character in "'\"":
            quote = character
            kept.append(" ")
            continue
        if character == quote:
            quote = None
            kept.append(" " if character == "'" else character)
            continue
        kept.append(" " if quote == "'" else character)
    return "".join(kept)


def substitutions_in(words, block):
    """(packages, references) from every substitution written in one line.

    Asked of the LINE and not of a command, because splitting at separators can
    cut a substitution in two: `cat <(echo ")"; rg …)` becomes `cat <(echo )`
    and `rg … )`, and the second is read as a case arm's pattern rather than a
    command (Codex, on c1c9438).
    """
    packages = set()
    references = set()

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
                undecided.add(
                    f"cannot tell which substitution in {word.value!r} is "
                    "literal and which runs"
                )
            continue
        inner += shell.substitution_bodies(word.value)

    # An unquoted `$( … )` splits into `$` and `(` too, so it is read from the
    # same rejoined line — leaving out the words whose DOLLAR is literal, which
    # is what the lexer already decided. Asking about whole-word quoting
    # instead lost `echo \\$(rg …)`, where two backslashes are one literal
    # backslash and the substitution after them still runs (Codex, on 92bf47b):
    # the word is quoted, and its dollar is not literal.
    inner += shell.substitution_bodies(
        "".join(
            (" " if word.space_before else "") + word.value
            for word in words
            if not word.literal_dollar
        )
    )

    # The rest is asked of the REJOINED line, because shlex gives neither
    # construct any meaning: `<(` arrives as `<` and `(`, and a backtick stays
    # welded to the name in front of it. The words are rejoined the way they
    # were written, so `< (` — a redirection and a subshell — cannot become a
    # process substitution on the way.
    #
    # A word holding a QUOTED bracket is left out: bash performs no process
    # substitution inside quotes of either kind, so `echo '<(rg …)'` prints
    # text (Codex, on 5fece60). EITHER bracket, because a quoted closer is data
    # too — in `cat <(echo ")"; rg …)` the substitution runs to the LAST
    # bracket, and keeping the quoted one closed it early and left the tool
    # outside (Codex, on c1c9438).
    #
    # Only a word holding a bracket, though. Dropping every quoted word threw
    # away the command name in `< <("rg" …)`, where the quotes are around a
    # word INSIDE a substitution that is itself unquoted and runs (Codex, on
    # dcf9333): quoting an argument is not quoting the construct.
    rendered = "".join(
        (" " if word.space_before else "") + word.value
        for word in words
        if not (word.quoted and any(bracket in word.value for bracket in "()"))
    )
    # `$(` is hidden first, so what comes back is only what the rewrite put
    # there. Without that, the rendered line — which keeps quoted words, for
    # the reason above — hands its own `'$(rg …)'` straight to a reader that
    # cannot tell it was quoted. The bracket stays where it is, so nothing's
    # balance changes, and the marker is put back in whatever is extracted in
    # case a process substitution holds a command substitution.
    hidden = rendered.replace("$(", MARKER + "(")
    inner += [
        body.replace(MARKER + "(", "$(")
        for body in shell.substitution_bodies(PROCESS_SUBSTITUTION.sub("$(", hidden))
    ]

    # Backticks are read from the LINE, not from the words. shlex marks an
    # escaped backtick and a single-quoted one alike — `\`rg` and `'`rg …`'`
    # both arrive quoted with `literal_backtick` set — so no word-level test
    # can tell the nested substitution from the inert one (Codex, on d32dd1d).
    # What separates them is where they sit, and single quotes are the only
    # thing that stops a backtick: double quotes do not.
    inner += ticked(PAIRED.sub(MARKER * 2, outside_single_quotes(block)))

    for body in inner:
        found, referenced = analysed(body)
        packages |= found
        references |= referenced
    return packages, references


def analysed_command(words, block=""):
    """(packages this one command runs, the repo scripts it runs)."""
    packages = set()
    references = set()

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
        wrapper = words[at].value.rsplit("/", 1)[-1]
        takes_argument = WRAPPER_OPTIONS_WITH_ARGUMENT.get(wrapper, frozenset())
        takes_letters = WRAPPER_LETTERS_WITH_ARGUMENT.get(wrapper, "")
        operands = WRAPPER_OPERANDS.get(wrapper, 0)
        position = at + 1
        while position < len(words):
            token = words[position].value
            if token.startswith("--"):
                token = resolved(token, wrapper)
            elif (
                len(token) > 1
                and token[0] == "-"
                and any(
                    letter in WRAPPER_TERMINAL_LETTERS.get(wrapper, "")
                    for letter in token[1:]
                )
            ):
                # `ionice -h rg`: a short help or version letter ends it too.
                return packages, references
            if token in TERMINAL_OPTIONS:
                # `xargs --help rg` prints usage and exits: the option ENDS the
                # invocation, and reading past it invented a command that never
                # runs (Codex, on 92bf47b).
                return packages, references
            if token.startswith("-"):
                # `xargs -I '{}' rg …`: the replacement string is the OPTION's,
                # and reading the first non-option word made it the command
                # (Codex, on 4e02d11). No quoting guard: the quotes are the
                # SHELL's, and xargs is handed `-I` either way (Codex, on
                # 951f419) — the same lesson as find's own `-exec`.
                if token.startswith("--"):
                    # `token` is the resolved spelling by now, so an
                    # abbreviation takes its value like the full one.
                    if token in takes_argument:
                        position += 1
                else:
                    # Short options cluster here as they do for a shell, but
                    # these are getopt's: a letter that takes a value takes it
                    # ATTACHED if anything follows it in the token, and only
                    # otherwise from the next word (Codex, on 1c9cf3e).
                    # `xargs -n1`, `xargs -rn1`, `xargs -I{}`, `timeout -k5`
                    # and `nice -n5` were each run to confirm it — bash's own
                    # `-o` is the exception, since it rejects the attached form
                    # outright, which is why that stays a count.
                    letters = token[1:]
                    for index, letter in enumerate(letters):
                        if letter not in takes_letters:
                            continue
                        if index + 1 == len(letters):
                            position += 1
                        break
                position += 1
                continue
            if operands:
                # `timeout 30s rg …`: a duration is an operand of timeout's
                # own, and it is not always a bare number.
                operands -= 1
                position += 1
                continue
            break
        if position >= len(words):
            break
        at = position
        # `timeout 30 env rg …`: what a wrapper resolves to may be a PREFIX,
        # which the command reader steps over at the FRONT of a command and
        # nothing re-applied here (Codex, on 951f419). Asking it again, of what
        # is left, is what makes the two rules compose.
        following = shell.command_word(words[at:])
        if following is None:
            break
        at += next(
            position for position, other in enumerate(words[at:]) if other is following
        )

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

    named = SCRIPT_REFERENCE.fullmatch(command.value)
    if named:
        # An executable script run by its own path. The PREFIX is dropped
        # here rather than at the far end: a reference is a repository path,
        # and `./x.sh`, `/abs/…/x.sh` and a `${{ }}`-masked one are all the
        # same file, which must not become three cache entries.
        references.add(named.group(1))
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
        while operand < len(words) and words[operand].value.startswith(("-", "+")):
            token = words[operand].value
            if token in TERMINAL_OPTIONS:
                # `bash --help script.sh` prints help and runs no script
                # (Codex, on e9ab98e; probed).
                return packages, references
            # Short options CLUSTER, and each `o` or `O` in one takes a word
            # of its own — ANYWHERE in the cluster, not only at the end, which
            # is what I first read into bash's usage line (Codex, on 5fece60
            # and again on 94be871). Checked against bash 5.2.21 rather than
            # inferred: `-oe pipefail script.sh` runs the script with both set,
            # `-oO pipefail nullglob script.sh` consumes two, and the attached
            # form `-opipefail` is rejected outright, so there is no case where
            # the value comes glued on.
            if token.startswith(("--", "++")):
                if token in shell.SHELL_OPTIONS_WITH_ARGUMENT:
                    operand += 1
            else:
                operand += sum(1 for letter in token[1:] if letter in "oO")
            operand += 1
        if operand < len(words):
            handed = SCRIPT_REFERENCE.fullmatch(words[operand].value)
            if handed:
                references.add(handed.group(1))
    return packages, references


def analysed(text):
    """(packages this shell text RUNS, the repo scripts it RUNS)."""
    packages = set()
    references = set()

    for block, words in blocks_of(text):
        found, referenced = substitutions_in(words, block)
        packages |= found
        references |= referenced
        for command in commands_in(words):
            found, referenced = analysed_command(command, block)
            packages |= found
            references |= referenced
    return packages, references


def worth_lexing(text):
    r"""Whether `text` could possibly name a tracked tool or a repo script.

    A cost filter, and ONLY for script bodies. Lexing is what normalises
    `r\g`, `r"g"` and `$'\x72g'` into `rg`, so a filter reading raw text is
    narrower than the reader behind it and ends up deciding verdicts by itself
    — which it did three times (Codex, on 07344b4 and again on 92bf47b). A
    workflow's own `run:` is small, so it is always lexed and this is not asked
    of it.

    LIMITATION: a script body is still filtered, because lexing every one costs
    minutes on this repository's own self-tests. The quoting characters come
    off first, so an assembled name is still found; a name spelled in ANSI-C
    quoting inside a SCRIPT would not be. Nothing here writes one, and the
    alternative is a gate too slow to run.
    """
    bare = QUOTING_CHARACTERS.sub("", text)
    return bool(
        any(candidate.search(bare) for candidate in CANDIDATES)
        or SCRIPT_REFERENCE.search(bare)
    )


def script_analysis(reference):
    """`analysed` for a repo script, read once however many jobs reach it."""
    if reference not in script_cache:
        path = os.path.join(REPO_ROOT, reference)
        try:
            with open(path, encoding="utf-8") as handle:
                body = handle.read()
        except OSError:
            # A workflow naming a script that is not there is a different fault,
            # and one the job reports loudly at `bash:` the first time it runs.
            body = ""
        # The text is kept as well as the verdict: the install side hands the
        # script to the apt scanner, and re-reading it there would be a second
        # answer to "what is in this file".
        script_bodies[reference] = body
        script_cache[reference] = analysed(body) if worth_lexing(body) else (set(), set())
    return script_cache[reference]


def tools_used(text):
    """(packages `text` needs, the repo scripts it runs) — both transitive."""
    needed, queue = analysed(text)
    seen = set()
    while queue:
        reference = queue.pop()
        if reference in seen:
            continue
        seen.add(reference)
        packages, references = script_analysis(reference)
        needed |= packages
        queue |= references - seen
    return needed, seen


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

needed_by_job = {}
for index, (path, job_name, job, _) in enumerate(jobs_found):
    needed_by_job[index] = set()
    for step in steps_of(job):
        if isinstance(step, dict) and isinstance(step.get("run"), str):
            needed, _followed = tools_used(step["run"])
            needed_by_job[index] |= needed

installed_by_job = installed_packages(jobs_found)

for index, (path, job_name, job, _) in enumerate(jobs_found):
    needed = needed_by_job[index]

    for package in sorted(needed - installed_by_job[index]):
        print(
            f"::error::{path}: job '{job_name}' runs a script that needs "
            f"{package}, which it never installs"
        )
        failures += 1

for what in sorted(undecided):
    print(f"notice: {what} — not checked")

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
