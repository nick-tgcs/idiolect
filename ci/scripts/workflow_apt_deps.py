#!/usr/bin/env python3
"""Extract the apt package names a CI definition installs.

Reads workflow and documentation files, and writes one tab-separated record per
finding to stdout:

    PKG<TAB>path<TAB>line<TAB>name        a package name to resolve
    NOTICE<TAB>path<TAB>line<TAB>text     something that cannot be resolved here

Resolving the names is the caller's job — see check-workflow-apt-deps.sh, which
also owns the apt guards and the exit status.

WHY THIS IS PYTHON AND NOT MORE BASH
------------------------------------
The first version of this scanner was hand-written string handling in bash, and
review found twenty-five defects in it across nine rounds. Almost every one was
the same mistake in different clothes: a whitespace tokeniser is not a shell
lexer, and a line reader is not a YAML parser. Comments; `apt-get [options]
install`; `sudo -E`; `cmake&&`; `bad-package>/dev/null`; `2>&1`; `&>`;
`$(printf '%s' g++)` spanning three tokens; `"cmake g++)"` as one argument;
`"codex;no-such-package"` where the `;` is literal; `run: >`, `>2`, `>2-`,
`- run: >`; lines more-indented than the fold baseline; blank lines inside a
fold; heredoc bodies; `<<'END-MARKER'`. Every one of those is something the
standard library already implements correctly.

So the two hard parts are delegated:

    shlex with punctuation_chars=True   quoting, comments, escapes, and the
                                        operators `;` `|` `&&` `>` `>&` `&>`,
                                        including the fact that a metacharacter
                                        inside quotes is a literal character.
    yaml.compose                        block scalars in every form, folding,
                                        indentation indicators, and the line
                                        each `run:` starts on.

What is left here is only the part specific to apt: where a package list begins
and ends. Keep it that way — if something in this file starts to look like
lexing or parsing, the library is being worked around rather than used.
"""

import sys

import yaml

# Options apt consumes an argument for, so the token after them is not a
# package. Only the ones apt accepts on `install`, each checked against apt
# 2.8.3: listing one that does NOT take an argument would swallow a real
# package, which is the failure this check exists to prevent.
OPTIONS_WITH_ARGUMENT = {
    "-o",
    "-c",
    "-t",
    "--option",
    "--config-file",
    "--target-release",
    "--default-release",
}

# `sudo -E apt-get ...` is documented sudo syntax, so the command may sit behind
# prefix words and their options.
INVOCATION_PREFIXES = {"sudo", "env"}

SEPARATORS = {";", "|", "&&", "||", "&", "\n"}

REDIRECTIONS = {">", "<", ">>", "<<<", ">&", "<&", "&>", "&>>", "<>"}


EXPRESSION_PREFIX = "__GITHUB_EXPRESSION_"
EXPRESSION_SENTINEL = EXPRESSION_PREFIX + "{}__"


def mask_expressions(text):
    """Replace `${{ ... }}` with single tokens. Returns (text, originals).

    shlex has no idea what a GitHub expression is, and `${{ matrix.pkg }}` is
    three whitespace-separated words of which the middle one looks exactly like
    a package name.
    """
    originals = []
    out = []
    index = 0
    while True:
        start = text.find("${{", index)
        if start < 0:
            out.append(text[index:])
            break
        end = text.find("}}", start)
        if end < 0:
            out.append(text[index:])
            break
        out.append(text[index:start])
        out.append(EXPRESSION_SENTINEL.format(len(originals)))
        originals.append(text[start:end + 2])
        index = end + 2
    return "".join(out), originals


def emit(kind, path, line, value):
    print(f"{kind}\t{path}\t{line}\t{value}")


def lex(text):
    """Tokenise one shell command line. Raises ValueError on an unclosed quote."""
    import shlex

    lexer = shlex.shlex(text, posix=True, punctuation_chars=True)
    lexer.whitespace_split = True
    return list(lexer)


def scan_command(path, line, tokens, expressions):
    """Report the packages installed by one already-tokenised command line."""
    state = "idle"
    # A command position: the start of the line, and again after every
    # separator. It survives the prefix material an invocation may carry, and
    # dies on the first token that is none of it — which is what stops "the apt
    # install step" in prose from being read as a command.
    at_command = True
    saw_apt = False
    saw_install = False
    parsed_install = False

    index = 0
    while index < len(tokens):
        token = tokens[index]
        index += 1

        if token in ("apt", "apt-get"):
            saw_apt = True
        elif token == "install":
            saw_install = True

        if token in SEPARATORS:
            state = "idle"
            at_command = True
            continue

        if token in REDIRECTIONS:
            # A redirection does not end an argument list — `cmd a >log b`
            # passes both a and b — so consume only its target.
            index += 1
            continue

        # `2>&1`: an all-digit word in front of a redirection is a file
        # descriptor, not a package.
        if token.isdigit() and index < len(tokens) and tokens[index] in REDIRECTIONS:
            continue

        if state == "packages":
            # `$(...)` arrives as `$`, `(`, the words inside, then `)`. None of
            # it can be resolved here, and a skip nobody can see is a skip
            # nobody can audit.
            if token == "$" and index < len(tokens) and tokens[index] == "(":
                # `$( ... )` arrives as `$`, `(`, its words, `)`. It is one
                # expansion and none of it can be resolved here, so it is
                # announced once rather than word by word.
                depth = 0
                parts = ["$("]
                index += 1
                depth = 1
                while index < len(tokens) and depth:
                    inner = tokens[index]
                    index += 1
                    if inner == "(":
                        depth += 1
                    elif inner == ")":
                        depth -= 1
                        if not depth:
                            break
                    parts.append(inner)
                rendered = "$(" + " ".join(parts[1:]) + ")"
                emit("NOTICE", path, line, f"names a package through a substitution, not checked: {rendered}")
                continue
            if token.startswith(EXPRESSION_PREFIX) and token.endswith("__"):
                position = token[len(EXPRESSION_PREFIX):-2]
                original = expressions[int(position)] if position.isdigit() else token
                emit("NOTICE", path, line, f"names a package through a variable, not checked: {original}")
                continue
            if "$" in token:
                emit("NOTICE", path, line, f"names a package through a variable, not checked: {token}")
                continue
            if token in OPTIONS_WITH_ARGUMENT:
                index += 1
                continue
            if token.startswith("-"):
                continue
            emit("PKG", path, line, token)
            continue

        if state == "options":
            # Everything between `apt-get` and its subcommand is an option; only
            # `install` opens a package list.
            if token == "install":
                state = "packages"
                parsed_install = True
            continue

        if not at_command:
            continue

        if token in ("apt", "apt-get"):
            state = "options"
        elif token in INVOCATION_PREFIXES or token.startswith("-") or "=" in token:
            pass
        else:
            at_command = False

    if saw_apt and saw_install and not parsed_install:
        # Not parsing a form is acceptable; not saying so is not, because a
        # silent skip reads exactly like a clean result.
        emit(
            "NOTICE",
            path,
            line,
            "looks like an apt install command but was not parsed as one — its packages were not checked",
        )


def join_continuations(lines):
    """Join `\\`-continued lines, keeping each result's first line number."""
    joined = []
    pending = None
    pending_index = 0
    for index, text in enumerate(lines):
        if pending is None:
            pending = ""
            pending_index = index
        pending = f"{pending} {text}" if pending else text
        if text.endswith("\\"):
            pending = pending[:-1]
            continue
        joined.append((pending_index, pending))
        pending = None
    if pending is not None:
        joined.append((pending_index, pending))
    return joined


def heredoc_delimiter(tokens):
    """The delimiter word of a heredoc opened on this line, if any."""
    for index, token in enumerate(tokens):
        if token in ("<<", "<<-") and index + 1 < len(tokens):
            return tokens[index + 1]
    return None


def scan_shell(path, text, first_line, exact_lines):
    """Report the packages installed by a block of shell.

    `exact_lines` says whether a physical line of `text` maps to a file line —
    true for a literal block or a plain scalar, false once YAML has folded the
    block, where every finding is reported against the line the block starts on.
    """
    heredoc = None
    for offset, command in join_continuations(text.splitlines()):
        line = first_line + offset if exact_lines else first_line

        if heredoc is not None:
            if command.strip() == heredoc:
                heredoc = None
            elif "apt" in command and "install" in command:
                # A heredoc body is data, not commands. Announced anyway, so a
                # misread delimiter cannot hide an install silently.
                emit(
                    "NOTICE",
                    path,
                    line,
                    f"is inside a heredoc, so it is data and not a command — not checked: {command.strip()}",
                )
            continue

        masked, expressions = mask_expressions(command)
        try:
            tokens = lex(masked)
        except ValueError as error:
            # Only worth reporting for a line that could hold an install; every
            # other unbalanced quote in a workflow is somebody else's business.
            if "apt" in command and "install" in command:
                emit("NOTICE", path, line, f"could not be tokenised ({error}), not checked: {command.strip()}")
            continue

        # Detected for EVERY line, not only the ones naming apt: the line that
        # opens a heredoc is usually `cat > file <<EOF`, which names nothing.
        heredoc = heredoc_delimiter(tokens)

        if "apt" in command:
            scan_command(path, line, tokens, expressions)


def shell_scalars(node):
    """Every scalar VALUE in a workflow, with the line it starts on and style.

    Not only `run:`. desktop-app-release.yml keeps an install in a matrix entry
    (`extra_deps: |`) and executes it later through `run: ${{ matrix.extra_deps }}`,
    so a scan that trusted the key name missed five packages — caught by
    comparing counts against the implementation this replaced.
    """
    if isinstance(node, yaml.MappingNode):
        for key, value in node.value:
            if isinstance(value, yaml.ScalarNode):
                yield value.start_mark.line + 1, value.style, value.value
            else:
                yield from shell_scalars(value)
    elif isinstance(node, yaml.SequenceNode):
        for child in node.value:
            if isinstance(child, yaml.ScalarNode):
                yield child.start_mark.line + 1, child.style, child.value
            else:
                yield from shell_scalars(child)


def scan_workflow(path, text):
    node = yaml.compose(text)
    if node is None:
        return
    for line, style, value in shell_scalars(node):
        # A plain scalar sits ON its line; a literal block keeps its lines, so
        # they follow the indicator; a folded block no longer has a line-for-line
        # mapping, so its findings are reported against the block itself.
        if style is None or style == "":
            scan_shell(path, value, line, exact_lines=True)
        elif style == "|":
            scan_shell(path, value, line + 1, exact_lines=True)
        else:
            scan_shell(path, value, line, exact_lines=False)


def main(argv):
    if not argv:
        print("usage: workflow_apt_deps.py <file> [file ...]", file=sys.stderr)
        return 2

    for path in argv:
        try:
            text = open(path, encoding="utf-8").read()
        except OSError as error:
            print(f"::error::{path} could not be read ({error})", file=sys.stderr)
            return 1

        if path.endswith((".yml", ".yaml")):
            try:
                scan_workflow(path, text)
            except yaml.YAMLError as error:
                # A file that does not parse is not a file with no packages.
                print(f"::error::{path} is not valid YAML ({error})", file=sys.stderr)
                return 1
        else:
            # Prose and documentation: no YAML structure, every line a candidate.
            scan_shell(path, text, 1, exact_lines=True)

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
