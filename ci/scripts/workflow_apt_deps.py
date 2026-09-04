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

import io
import re
import shlex
import string
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
# prefix words and their options. `command` and `exec` are the shell's own:
# `command` runs its argument bypassing functions and aliases, `exec` replaces
# the shell with it, and either way apt runs and the packages are its.
#
# `builtin` is deliberately absent — it runs BUILTINS only, so `builtin apt-get`
# fails and checking its packages would be a red on something that cannot run.
INVOCATION_PREFIXES = {"sudo", "env", "command", "exec"}

# Reserved words and pipeline prefixes that INTRODUCE a command rather than
# being one: bash runs what comes after them, so `if ${{ env.command }}; then`
# runs the value exactly as a bare `${{ env.command }}` would.
CONTROL_PREFIXES = {
    "if", "then", "elif", "else", "while", "until", "do", "!", "time", "{", "(",
}

APT_COMMANDS = {"apt", "apt-get"}

# What an array's bracket is written against: `deps=(` or `deps+=(`. A word
# merely CONTAINING an `=` is not one — `RESULT=$(` ends in a dollar, and bash
# runs the substitution that opens there.
ARRAY_PREFIX = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*\+?=$")


# `"${deps[@]}"` is the whole array as separate words. Only THERE do its
# elements become a command; the initializer says what the words are, not that
# they will ever be run.
ARRAY_EXPANSION = re.compile(r"^\$\{([A-Za-z_][A-Za-z0-9_]*)\[[@*]\]\}$")


# Shells whose `-c` argument is a script rather than an operand.
SHELL_COMMANDS = {"bash", "sh", "dash", "zsh", "ksh"}

# Options a shell takes an argument for, so the word after one is not the
# script FILE that ends the invocation's options.
SHELL_OPTIONS_WITH_ARGUMENT = {"-o", "+o", "--rcfile", "--init-file"}


def hands_over_a_script(word):
    """Whether this option makes the word after it a script rather than a file.

    `-c`, and the clustered short forms bash accepts for it: `bash -ec '…'`,
    `-ce` and `-euxc` all run the script. A LONG option is not a cluster —
    `--config` is one word, not six flags — and a cluster with no `c` in it
    hands over nothing, `bash -e file` naming a file to run.
    """
    token = word.value
    return (
        not word.quoted
        and len(token) > 1
        and token[0] == "-"
        and token[1] != "-"
        and "c" in token[1:]
    )


def runs_a_script(words):
    """Whether these words hand a script to a shell — `bash -c '…'`, `eval '…'`.

    Asked because a script arrives as ONE quoted word, so nothing in it lexes
    as apt and the line would otherwise never be looked at. Deliberately broad:
    it decides only whether to READ the line, and `script_argument` decides
    which word the script actually is.

    No quoting guard on the name: `'bash' -c '…'` runs bash. Quotes take a
    reserved word's meaning away and leave a command name as it was.
    """
    names = [word.value.rsplit("/", 1)[-1] for word in words]
    if "eval" in names:
        return True
    return any(name in SHELL_COMMANDS for name in names) and any(
        hands_over_a_script(word) for word in words
    )


# The forms a bare shell variable takes when it supplies a command: `$NAME` and
# `${NAME}`, and nothing more — an index, a default or a substring makes the
# value something other than what the workflow wrote down.
#
# The braces are written as two alternatives rather than one optional pair,
# because independently optional braces match `$NAME}` — which bash expands and
# then appends the `}` to, asking apt for a package that does not exist.
SHELL_VARIABLE = re.compile(
    r"^\$([A-Za-z_][A-Za-z0-9_]*)$|^\$\{([A-Za-z_][A-Za-z0-9_]*)\}$"
)


def is_an_assignment(word):
    """Whether this word sets a variable rather than naming a command.

    `NAME=value` and `NAME+=value`; a word that STARTS with `=` assigns
    nothing. Quoting is not asked about, because it is per WORD here and an
    assignment is routinely written `OUT="…"` — only the characters before the
    `=` would have to be unquoted for bash, and a fully quoted `"OUT=x"` names
    a command rather than assigning. That form appears in no workflow, and
    reading it as an assignment costs a command name rather than a package.
    """
    return "=" in word.value and not word.value.startswith("=")


def is_apt(token):
    """Whether this word names apt, however it is spelled.

    `/usr/bin/apt-get` is the same command as `apt-get`. Matching the name
    exactly missed it entirely — and because the "looks like an apt install
    command" safety net matched exactly too, its packages went unexamined AND
    unmentioned, which is the one combination this scanner must never produce.
    """
    return token.rsplit("/", 1)[-1] in APT_COMMANDS

SEPARATORS = {";", "|", "&&", "||", "&", "\n"}

# `<<` belongs here as well as to the heredoc tracking: `apt-get install -y
# cmake <<EOF` redirects a heredoc INTO apt, so neither the operator nor the
# delimiter after it is a package.
REDIRECTIONS = {">", "<", ">>", "<<", "<<<", ">&", "<&", "&>", "&>>", "<>"}

# shlex groups a RUN of punctuation into one token, so `);` and `)>` arrive
# welded together and match neither the parenthesis nor the operator. Longest
# first, because `<<<` must not be read as `<<` and then `<`. `;;` is absent on
# purpose: split into two separators it ends a command list, which is what it
# does, where kept whole it is neither operator nor package.
OPERATORS = (
    "&>>", "<<<", ">>", "<<", ">&", "<&", "&>", "<>", "&&", "||",
    ";", "|", "&", "<", ">", "(", ")",
)

# Taken from shlex rather than restated, so the two cannot drift apart.
PUNCTUATION = set(shlex.shlex("", punctuation_chars=True).punctuation_chars)


def split_operators(token):
    """Split a welded run of punctuation into the operators it is made of."""
    if not token or not set(token) <= PUNCTUATION:
        return [token]
    parts = []
    index = 0
    while index < len(token):
        for operator in OPERATORS:
            if token.startswith(operator, index):
                parts.append(operator)
                index += len(operator)
                break
        else:
            # Unreachable while every punctuation character is a one-character
            # operator above, and load-bearing if that ever stops being true:
            # without it the loop would not advance.
            parts.append(token[index])
            index += 1
    return parts


# What a `$` must be followed by to begin an expansion: a name, `{`, `(`, or one
# of the special parameters. Anything else — including the end of the word —
# leaves it an ordinary character, and `codex-no-such-package$` is a name apt
# rejects rather than a variable to excuse.
EXPANSION_INTRODUCERS = frozenset(string.ascii_letters + string.digits + "_{(@*#?-!$")


def expands_a_dollar(token):
    """Whether any `$` in this word begins a shell expansion."""
    return any(
        char == "$" and position + 1 < len(token) and token[position + 1] in EXPANSION_INTRODUCERS
        for position, char in enumerate(token)
    )


def expands_a_brace(token):
    """Whether bash would brace-expand this word.

    Only where there is something to expand: `lib{asound2,pulse}-dev` becomes
    two package names, while `lib{only}-dev` has no comma and no range and is
    passed through unchanged — so excusing every brace would hide a name apt
    rejects.
    """
    start = token.find("{")
    while start != -1:
        end = token.find("}", start + 1)
        if end != -1 and ("," in token[start + 1:end] or ".." in token[start + 1:end]):
            return True
        start = token.find("{", start + 1)
    return False


# apt reads an argument holding a `/` or ending in `.deb` as a local package
# FILE. Debian names contain neither, so a path is never a name — but
# `pkg/stable` is the target-release form rather than a path, and stays a name.
def names_a_file(word):
    """Whether apt would read this argument as a local .deb rather than a name.

    apt decides that from the text — a `/` or a `.deb` suffix — which quoting
    does not change. The TILDE is different, because it is the shell that
    expands it: quoted it is never expanded, and unquoted `~name` expands only
    if that user exists, bash leaving it alone otherwise. Only `~/` is certainly
    a home directory, so only that counts here; anything else keeps its tilde
    and is a name apt will reject.
    """
    token = word.value
    if token.endswith(".deb") or token.startswith(("/", "./", "../")):
        return True
    return token.startswith("~/") and not word.quoted


# `*`, `?` and `[` make a word a pattern — expanded by the shell if it matches a
# path, and treated as a package pattern by apt itself if it does not. Either
# way this script cannot say which names it stands for.
GLOB_CHARACTERS = frozenset("*?[")


def expands_a_glob(token):
    """Whether this word is a pattern rather than a single name."""
    return any(character in GLOB_CHARACTERS for character in token)


EXPRESSION_PREFIX = "__GITHUB_EXPRESSION_"
WHOLE_EXPRESSION = re.compile(r"^__GITHUB_EXPRESSION_\d+__$")


def is_partly_assembled(token):
    """Whether this word is part literal and part expression.

    `a${{ '' }}pt-get` runs apt-get, but the word is neither `apt-get` nor a
    bare expression, so it was neither checked NOR announced. A word the
    workflow builds out of both is assembled at run time like any other.
    """
    return EXPRESSION_PREFIX in token and not WHOLE_EXPRESSION.match(token)
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


def unmask(text, expressions):
    """Put the original `${{ ... }}` back wherever a sentinel stands in."""
    for position, original in enumerate(expressions):
        text = text.replace(EXPRESSION_SENTINEL.format(position), original)
    return text


def emit(kind, path, line, value):
    print(f"{kind}\t{path}\t{line}\t{value}")


class Word:
    """One shell word: what apt would receive, and how it was quoted.

    `literal_dollar` is true when the word holds a `$` the shell does NOT
    expand — inside single quotes, or backslash-escaped. That word reaches apt
    with the dollar sign in it, so it is a package name to resolve and not a
    variable to excuse.

    `quoted` is true when any part of the word was quoted or escaped, which is
    what makes it a WORD and not an operator: bash passes `'<<'` and `';'` to
    the command as arguments, and shlex hands back tokens indistinguishable
    from the real operators.

    `space_before` is false when no whitespace separates this word from the one
    before it. shlex splits `<<-EOF` and `<< -EOF` into the same three tokens,
    and `c$(printf make)` into five, so adjacency is the only thing that says
    whether a dash belongs to the operator or to the delimiter, and whether a
    fragment belongs to the word beside it.
    """

    __slots__ = ("value", "literal_dollar", "space_before", "literal_backtick",
                 "quoted", "literal_brace")

    def __init__(self, value, literal_dollar=False, space_before=True,
                 literal_backtick=False, quoted=False, literal_brace=False):
        self.value = value
        self.literal_dollar = literal_dollar
        self.space_before = space_before
        self.literal_backtick = literal_backtick
        self.quoted = quoted
        self.literal_brace = literal_brace

    def __repr__(self):
        return (f"Word({self.value!r}, literal_dollar={self.literal_dollar}, "
                f"space_before={self.space_before}, "
                f"literal_backtick={self.literal_backtick}, quoted={self.quoted}, "
                f"literal_brace={self.literal_brace})")


class _QuoteWatchingStream(io.StringIO):
    """Feeds shlex, and notes the quoting in force as each character is read.

    shlex removes quotes and escapes, so by the time a token exists `'$X'`,
    `\\$X` and `$X` are the same three characters and the distinction that
    decides whether apt sees a literal dollar is gone. Rather than reconstruct
    it — re-deriving quoting by hand is exactly what this file exists not to do
    — the answer is taken from the lexer while it still has it: `shlex.state`
    holds the quote character it is inside, or the escape character it has just
    consumed. Which state means what is pinned by a test, so a change in a
    future Python fails loudly instead of silently reopening the hole.
    """

    def __init__(self, text):
        super().__init__(text)
        self.lexer = None
        self.literal_dollar = False
        self.literal_backtick = False
        self.quoted = False
        self.literal_brace = False
        self.last_char = None
        self.last_was_escaped = False
        self.position = 0
        self.dollar_quotes = []
        self.dollar_quote_opened = None
        self.dollar_quote_escaped = False

    def read(self, size=-1):
        char = super().read(size)
        if char:
            state = self.lexer.state
            if (
                char in "'\""
                and self.last_char == "$"
                and not self.last_was_escaped
                and state not in ("'", '"')
                # Not while one is already open: past an escaped quote shlex's
                # state no longer says we are inside a string, so a `$` before
                # the real closing quote would read as a second `$'` opening
                # and carry the span's start past the name it holds.
                and self.dollar_quote_opened is None
            ):
                # `$'...'` and `$"..."` are bash's own QUOTING forms, not
                # expansions: the contents are passed WITHOUT the dollar. shlex
                # removes the quotes and leaves the `$` welded to the text,
                # where it reads exactly like a parameter expansion — so the
                # span is recorded and rewritten before lexing again.
                self.dollar_quote_opened = (self.position - 1, char)
            elif (
                self.dollar_quote_opened is not None
                and char == self.dollar_quote_opened[1]
                and not self.dollar_quote_escaped
            ):
                # The closing quote of one: the span is `$'...'` inclusive.
                self.dollar_quotes.append((self.dollar_quote_opened[0], self.position))
                self.dollar_quote_opened = None
            if self.dollar_quote_opened is not None:
                # Inside either form a backslash escapes the next character,
                # the closing quote included — `$'a\'b'` is one word. Asking
                # bash's rule rather than shlex's state, because for `$'...'`
                # the two have already parted company: ordinary single quotes
                # have no escapes, so shlex ended the string at that quote and
                # is lexing the rest as if it were outside one.
                self.dollar_quote_escaped = char == "\\" and not self.dollar_quote_escaped
            else:
                self.dollar_quote_escaped = False
            if state in ("'", '"', self.lexer.escape):
                self.quoted = True
                if char in "{},.":
                    # Quoting the braces, the comma, or a dot of a `..` range
                    # suppresses the expansion; quoting one ALTERNATIVE does
                    # not, so a word-level flag says the wrong thing about
                    # `lib{asound2,"pulse"}-dev`.
                    #
                    # APPROXIMATION, and the direction matters: the flag is per
                    # WORD, so a quoted character anywhere in it makes the whole
                    # word literal. `lib{a,b}"."c` would be resolved verbatim
                    # and rejected where bash expands it — a false red on an
                    # input no workflow writes, in exchange for not excusing
                    # `codex{1".".2}`, which bash passes to apt whole.
                    self.literal_brace = True
            if char in "$`" and self.lexer.state in ("'", self.lexer.escape):
                # Single quotes and a backslash suppress BOTH forms of
                # expansion; double quotes suppress neither.
                if char == "$":
                    self.literal_dollar = True
                else:
                    self.literal_backtick = True
            self.last_was_escaped = state == self.lexer.escape
            self.last_char = char
            self.position += 1
        return char


class Unlexable(ValueError):
    """An unbalanced quote, carrying the words read before it.

    `state` is the quote character still open when the lexer gave up, which is
    what says whether a trailing backslash was an escape or an ordinary
    character inside single quotes.
    """

    def __init__(self, error, words, state, dollar_quotes):
        super().__init__(str(error))
        self.words = words
        self.state = state
        # The `$'...'` spans closed before the failure. One of them may BE the
        # reason for it, so the caller can rewrite them and lex again.
        self.dollar_quotes = list(dollar_quotes)


def _lex(text, commenters, dollar_quotes=None):
    """Tokenise, with shlex's own comment handling set to `commenters`.

    Raises Unlexable on an unbalanced quote, carrying the words already read —
    which is what makes a comment containing an apostrophe recoverable without
    re-lexing the line under weaker rules.
    """
    stream = _QuoteWatchingStream(text)
    lexer = shlex.shlex(stream, posix=True, punctuation_chars=True)
    stream.lexer = lexer
    lexer.whitespace_split = True
    lexer.commenters = commenters

    words = []
    while True:
        # Reset per word: the characters read while producing this token are
        # this token's, since shlex consumes the whitespace that ends a word
        # before returning it and pushes punctuation back unread.
        stream.literal_dollar = False
        stream.literal_backtick = False
        stream.quoted = False
        stream.literal_brace = False
        # The character last read is either the whitespace that ended the word
        # before, or — when shlex pushed one back — the first character of this
        # word. Either way, whitespace here means the two words are separated.
        space_before = stream.last_char is None or stream.last_char.isspace()
        try:
            token = lexer.get_token()
        except ValueError as error:
            raise Unlexable(error, words, lexer.state, stream.dollar_quotes) from error
        if token is lexer.eof:
            if dollar_quotes is not None:
                dollar_quotes.extend(stream.dollar_quotes)
            return words
        if stream.quoted:
            # A quoted run of punctuation is an argument, not a sequence of
            # operators, so it is not taken apart.
            words.append(Word(token, stream.literal_dollar, space_before,
                              stream.literal_backtick, True, stream.literal_brace))
            continue
        for position, part in enumerate(split_operators(token)):
            # Only the first part can have had whitespace before it; the rest
            # were welded to their predecessor by definition.
            words.append(Word(part, stream.literal_dollar,
                              space_before and position == 0, stream.literal_backtick,
                              False, stream.literal_brace))


# Where a new word can begin without any whitespace: after an operator.
WORD_BOUNDARIES = SEPARATORS | REDIRECTIONS | {"(", ")"}


def without_dollar_quoting(text, spans):
    """Rewrite each `$'...'` / `$"..."` span as the text bash would pass on.

    `$'...'` is not ordinary single quoting: it DECODES backslash escapes, so
    `$'c\\x6dake'` is the package `cmake`. Removing the dollar alone leaves a
    literal that apt cannot resolve. `$"..."` translates rather than decodes,
    so there the dollar simply goes.

    The decoded text is requoted by shlex rather than wrapped in the quote it
    came in: it may now CONTAIN that quote — `$'a\\'b'` decodes to `a'b` — and
    putting it back between apostrophes would produce a line no lexer can read.
    """
    rewritten = []
    end_of_last = 0
    for start, finish in sorted(spans):
        rewritten.append(text[end_of_last:start])
        quote = text[start + 1]
        content = text[start + 2:finish]
        if quote == "'":
            try:
                content = content.encode("utf-8", "surrogateescape").decode("unicode_escape")
            except (UnicodeDecodeError, ValueError):
                # An escape python does not know: leave it as written rather
                # than invent a name.
                pass
            rewritten.append(shlex.quote(content))
        else:
            rewritten.append(quote + content + quote)
        end_of_last = finish + 1
    rewritten.append(text[end_of_last:])
    return "".join(rewritten)


def starts_a_comment(words, index):
    """Whether this word is where bash would start a comment.

    `#` opens a comment only where a WORD can begin. After other characters it
    is an ordinary character — `cmake#typo` is a package name and apt rejects
    it — and quoted it is a package name of its own. An operator ends a word as
    surely as a space does, so `bad-package;#comment` is a comment too.
    """
    word = words[index]
    if not word.value.startswith("#") or word.quoted:
        return False
    if word.space_before or index == 0:
        return True
    previous = words[index - 1]
    return not previous.quoted and previous.value in WORD_BOUNDARIES


def lex_words(text):
    """Tokenise one shell command line. Raises ValueError on an unclosed quote.

    Comments are applied by bash's rule rather than shlex's, which treats every
    unquoted `#` as one wherever it appears.
    """
    dollar_quotes = []
    try:
        words = _lex(text, commenters="", dollar_quotes=dollar_quotes)
    except Unlexable as error:
        if error.dollar_quotes:
            # The apostrophe shlex gave up on may be one bash never ended a
            # string with: `$'a\'b'` closes at the LAST quote. Rewriting the
            # spans removes the form shlex cannot read, so the line gets a
            # second, honest reading rather than a notice standing in for one.
            return lex_words(without_dollar_quoting(text, error.dollar_quotes))
        # A comment may hold an apostrophe — `# don't` is an ordinary line — and
        # no lexer can read that as shell. But everything after a `#` is comment
        # anyway, so if one was reached before the quote, the words already read
        # ARE the command and nothing is missing. Only when no comment was
        # reached is the quote a real defect in the command itself.
        #
        # Re-lexing the line under shlex's own comment rules would get past the
        # apostrophe too, and would truncate `cmake#typo` to `cmake` on the way.
        words = error.words
        if not any(starts_a_comment(words, index) for index in range(len(words))):
            raise
    else:
        if dollar_quotes:
            return lex_words(without_dollar_quoting(text, dollar_quotes))
    for index in range(len(words)):
        if starts_a_comment(words, index):
            return words[:index]
    return words


def continues_the_line(text):
    r"""Whether a trailing backslash continues this line onto the next.

    Backslashes come in pairs: `foo\\` is one ESCAPED backslash and the line
    ends there, while `foo\\\` is an escaped one followed by a continuation.
    Only an ODD number of them at the end continues anything, and testing the
    final character alone joins two commands that bash runs separately.

    Parity is not the whole rule. Inside SINGLE quotes a backslash is an
    ordinary character, so `'cma\` continues nothing — the string simply
    carries a backslash and a newline, and apt is handed a name it rejects.
    Whether the line ends inside single quotes is the lexer's own answer: it
    reports the quote still open when it gives up.
    """
    if (len(text) - len(text.rstrip("\\"))) % 2 == 0:
        return False
    try:
        # Without the trailing backslashes, since an escape with nothing after
        # it is an error of its own and would mask the quote.
        _lex(text.rstrip("\\"), commenters="")
    except Unlexable as error:
        if error.state == "'":
            return False
    return True


def ends_inside_a_quote(text):
    """The quote character still open at the end of this text, or None.

    Bash carries an open quote across the newline, so a word may span physical
    lines and the newline is part of it. Scanning each line alone turns that
    into an untokenisable fragment and loses the command.
    """
    try:
        _lex(text, commenters="")
    except Unlexable as error:
        if error.state in ("'", '"'):
            return error.state
    return None


def holds_a_comment(text):
    """Whether an unquoted `#` starts a comment somewhere in this line.

    Decided by lexing twice — once with shlex dropping comments, once with it
    keeping them — because that is the one place the answer already exists.
    Quoting is what makes the question hard — a `#` inside quotes starts no
    comment — and shlex is the thing that already knows about quoting.
    """
    try:
        words = _lex(text, commenters="")
    except Unlexable as error:
        # The words read BEFORE the quote still answer the question, and a
        # comment holding an apostrophe is exactly the line where it matters:
        # answering "no comment" here joins it to the next one and the command
        # beneath disappears into it.
        words = error.words
    return any(starts_a_comment(words, index) for index in range(len(words)))


def walk_to_closing_paren(words, index):
    """Consume a parenthesised group; returns its words and the index after it.

    `index` must point AT the opening `(`. Nesting is counted, so `$(a $(b) c)`
    ends on its own bracket rather than the first one it meets — but only
    UNQUOTED brackets nest. A `(` inside quotes is an ordinary character, and
    counting it runs the walk past the real closing bracket and swallows the
    operands after it.
    """
    depth = 1
    index += 1
    inside = []
    while index < len(words) and depth:
        word = words[index]
        index += 1
        if not word.quoted:
            if word.value == "(":
                depth += 1
            elif word.value == ")":
                depth -= 1
                if not depth:
                    break
        inside.append(word)
    return inside, index


def command_ends(words, start):
    """Where the command beginning at `start` ends: the next separator, or the end."""
    for index in range(start, len(words)):
        if words[index].value in SEPARATORS and not words[index].quoted:
            return index
    return len(words)


def case_pattern_end(words):
    """Where a case ARM's pattern ends, or None if these words are not one.

    An arm is `pattern ) list` and bash runs the list: the pattern and its
    parenthesis are syntax. What tells one from a subshell is that the arm's
    `)` closes nothing — in `( cmd )` the parenthesis has an opener, and comes
    after what runs rather than before it.

    Asked of a SEGMENT, which is split at separators, so the one arm in view
    has at most one pattern. The literal scan walks whole lines instead and
    reads every `)` as it reaches it, since `x) …;; y) …;;` is one line with
    two arms in it.
    """
    depth = 0
    for position, word in enumerate(words):
        if word.quoted:
            continue
        if word.value == "(":
            depth += 1
        elif word.value == ")":
            if depth == 0:
                return position
            depth -= 1
    return None


def scan_command(path, line, words, expressions, defined=None):
    """Report the packages installed by one already-tokenised command line.

    `defined` carries what earlier commands in the same block have DEFINED —
    arrays, variables and functions — because a definition and the command
    that runs it are two commands apart, and declaring one runs none of it.
    """
    state = "idle"
    # A command position: the start of the line, and again after every
    # separator. It survives the prefix material an invocation may carry, and
    # dies on the first token that is none of it — which is what stops "the apt
    # install step" in prose from being read as a command.
    at_command = True
    saw_apt = False
    saw_install = False
    parsed_install = False
    pending = []
    end_of_options = False

    index = 0
    while index < len(words):
        word = words[index]
        token = word.value
        index += 1

        if is_apt(token):
            saw_apt = True
        elif token == "install":
            saw_install = True

        if token in SEPARATORS and not word.quoted:
            remember(pending if at_command else [], defined, expressions)
            pending = []
            state = "idle"
            at_command = True
            end_of_options = False
            continue

        if token == ")" and not word.quoted:
            # A closing bracket is syntax either way, and either way a command
            # may follow it: a case ARM's `)` ends its pattern, and a
            # subshell's ends the list inside it. One rule, because a line may
            # hold several arms — `x) …;; y) …;;` — and remembering a single
            # position covered only the first.
            #
            # The brackets that belong to a substitution or an array never
            # reach here: those are consumed whole, above.
            state = "idle"
            at_command = True
            end_of_options = False
            continue

        assignment = assigned_array(words, index - 1)
        if assignment is not None:
            # `deps=(cmake g++)` is an ARRAY assignment, not a subshell. Reading
            # the parenthesis as a command position — which is what lets
            # `( apt-get … )` work — made apt inside one a command whose last
            # package was the closing bracket, and rejected a working workflow
            # for a package called `)`.
            #
            # Its elements are REMEMBERED rather than read as a command: an
            # initializer says what the words are, not that they will ever be
            # run, and `printf '%s\n' "${deps[@]}"` only prints them.
            prefix, inside, index = assignment
            if defined is not None:
                remember_array(prefix, inside, defined, expressions)
            continue

        if (
            token in ("<", ">")
            and not word.quoted
            and index < len(words)
            and words[index].value == "("
            and not words[index].space_before
        ):
            # `<(cmd)` is process substitution, not a redirection: bash replaces
            # the whole construct with a /dev/fd path. Consuming only the `(`
            # left the command inside it being read as a package list.
            inside, index = walk_to_closing_paren(words, index)
            if state == "packages":
                substituted = " ".join(word.value for word in inside)
                emit("NOTICE", path, line,
                     f"names a package through a substitution, not checked: {token}({substituted})")
            continue

        if token in REDIRECTIONS and not word.quoted:
            # A redirection does not end an argument list — `cmd a >log b`
            # passes both a and b — so consume only its target.
            if (
                token == "<<"
                and index < len(words)
                and words[index].value == "-"
                and not words[index].space_before
            ):
                # `<<- EOF`: the dash is the operator's, and the delimiter is
                # the word after it.
                index += 1
            index += 1
            continue

        # `2>&1`: an all-digit word in front of a redirection is a file
        # descriptor, not a package.
        if (
            token.isdigit()
            and not word.quoted
            and index < len(words)
            and words[index].value in REDIRECTIONS
            and not words[index].quoted
            and not words[index].space_before
        ):
            # Attached only: `2>out` redirects, while `2 >out` passes `2` as an
            # argument and apt is asked for a package called `2`.
            continue

        if state == "packages":
            if "`" in token and not word.literal_backtick:
                # `` c`printf make` `` is one word that expands to `cmake`.
                # Backticks are not special to shlex, so the substitution
                # arrives split across whatever whitespace it contains; it is
                # re-joined here and announced once, rather than its fragments
                # being reported as package names.
                rendered = token
                ticks = token.count("`")
                while ticks % 2 and index < len(words):
                    following = words[index]
                    rendered += (" " if following.space_before else "") + following.value
                    ticks += following.value.count("`")
                    index += 1
                emit("NOTICE", path, line, f"names a package through a substitution, not checked: {rendered}")
                continue
            if (
                token.endswith("$")
                and not word.literal_dollar
                and index < len(words)
                and words[index].value == "("
                and not words[index].space_before
            ):
                # `$( ... )` arrives as `$`, `(`, its words, `)`. It is one
                # expansion and none of it can be resolved here, so it is
                # announced once rather than word by word. Quoted, it would not
                # be an expansion at all and would arrive as a single word —
                # which is why this branch cannot be reached by `'$(x)'`.
                #
                # The substitution need not be the whole argument, either:
                # `c$(printf make)` is one word that expands to `cmake`, and
                # apt installs it. Reporting the fragments around it as package
                # names rejects a workflow that works.
                inside, index = walk_to_closing_paren(words, index)
                rendered = token[:-1] + "$(" + " ".join(w.value for w in inside) + ")"
                # Whatever is written hard against the closing `)` is part of
                # the same word — but a separator or a redirection is not, and
                # absorbing one would discard the command after it.
                while (
                    index < len(words)
                    and not words[index].space_before
                    and words[index].value not in SEPARATORS
                    and words[index].value not in REDIRECTIONS
                ):
                    rendered += words[index].value
                    index += 1
                emit("NOTICE", path, line, f"names a package through a substitution, not checked: {rendered}")
                continue
            if EXPRESSION_PREFIX in token:
                # An expression need not be the whole argument:
                # `lib${{ matrix.flavor }}` is one package name the workflow
                # builds at run time. Judging only whole-word expressions
                # reports the sentinel as a package, and apt cannot resolve
                # something this scanner invented.
                emit("NOTICE", path, line,
                     f"names a package through a variable, not checked: {unmask(token, expressions)}")
                continue
            if expands_a_dollar(token) and not word.literal_dollar:
                emit("NOTICE", path, line, f"names a package through a variable, not checked: {token}")
                continue
            if names_a_file(word):
                emit("NOTICE", path, line,
                     f"names a local package file, not a name to resolve: {token}")
                continue
            if expands_a_glob(token):
                emit("NOTICE", path, line,
                     f"names packages through a pattern, not checked: {token}")
                continue
            if expands_a_brace(token) and not word.literal_brace:
                # Announced rather than expanded. Expanding would check more —
                # both halves of `lib{asound2,pulse}-dev` exist — but brace
                # expansion is a grammar of its own, with ranges and nesting,
                # and the standard library does not implement it. Writing that
                # by hand is the mistake this file was rewritten to undo. Spell
                # the packages out in the workflow and the gate checks them.
                emit("NOTICE", path, line,
                     f"names packages through a brace expansion, not checked: {token}")
                continue
            if not end_of_options:
                if token == "--":
                    # Everything after this is an operand, however it starts:
                    # apt rejects `-codex-no-such-package` as a package name
                    # rather than reading it as an option.
                    end_of_options = True
                    continue
                if token in OPTIONS_WITH_ARGUMENT:
                    index += 1
                    continue
                if token.startswith("-"):
                    continue
            # A literal `$` falls through to here on purpose: the shell expands
            # nothing inside single quotes or after a backslash, so apt is
            # handed the dollar sign and rejects the name. That is a broken
            # install, not an unresolvable one.
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

        if is_apt(token):
            state = "options"
        elif is_an_assignment(word):
            # Assignments first: `TAG=${{ inputs.tag }}` sets a variable and
            # names no command, and announcing those flagged two lines of this
            # repository's own workflows. Remembered only if nothing runs after
            # it in this command: `FLAG=1 cmd` sets it for cmd alone.
            pending.append(word)
        elif token in INVOCATION_PREFIXES or token.startswith("-"):
            pass
        elif token in CONTROL_PREFIXES and not word.quoted:
            # A reserved word INTRODUCES a command: `if apt-get install -y x;
            # then` installs x, and so does `( apt-get ... )`. Ending the
            # command position here left a real install announced as unparsed —
            # a notice, which nothing fails on. Quoted it is no longer reserved:
            # bash looks for a command called `if` and never runs what follows.
            pass
        elif defined is not None and defined_function(words, index - 1) is not None:
            # Declaring a function runs none of it: the body is remembered and
            # read where the name is CALLED.
            name, body, index = defined_function(words, index - 1)
            defined[("function", name)] = (body, expressions)
            at_command = True
        elif defined is not None and remembered(token, defined) is not None:
            # `"${deps[@]}"`, `$COMMAND` and a function's name each put what
            # was remembered HERE, at a command position. A function takes its
            # arguments as `$1`; the other two are words in this command, so
            # whatever follows belongs to them.
            key = remembered(token, defined)
            end = len(words) if key[0] == "function" else command_ends(words, index)
            after = [] if key[0] == "function" else words[index:end]
            elsewhere = {other: held for other, held in defined.items() if other != key}
            scan_command(path, line, rebased(*defined[key], expressions) + after,
                         expressions, elsewhere)
            index = end if key[0] != "function" else index
            at_command = False
        elif token.rsplit("/", 1)[-1] in SHELL_COMMANDS or token.rsplit("/", 1)[-1] == "eval":
            # `bash -c '…'` and `eval '…'` run their argument as a SCRIPT, and
            # apt inside one installs for real. Scanned rather than announced:
            # it is shell, and this file already knows how to read shell.
            #
            # WHICH word is the script is asked of `script_argument`, the one
            # the expression walk asks too — the options a shell reads stop at
            # its script FILE, and holding that rule in two places is how
            # `bash build.sh -c x` came to be read as an invocation option.
            script = script_argument(words[index - 1:command_ends(words, index - 1)])
            if script is not None:
                scan_shell(path, script.value, line, exact_lines=False)
            at_command = False
        elif is_partly_assembled(token):
            # Announced, never silent: the name of the command being run does
            # not appear in the file.
            emit("NOTICE", path, line,
                 f"builds its command name with an expression, assembled at run time "
                 f"and not checked: {unmask(token, expressions)}")
            at_command = False
        else:
            at_command = False

    remember(pending if at_command else [], defined, expressions)

    if saw_apt and saw_install and not parsed_install:
        # Not parsing a form is acceptable; not saying so is not, because a
        # silent skip reads exactly like a clean result.
        emit(
            "NOTICE",
            path,
            line,
            "looks like an apt install command but was not parsed as one — its packages were not checked",
        )


def heredocs_opened(words):
    """Every heredoc a command opens, in order, as (delimiter, tabs_stripped).

    In order, and all of them: `cat <<A <<B` reads A's body and then B's, so a
    scanner that keeps only the first resumes reading commands while the shell
    is still reading data.

    The dash in `<<-EOF` belongs to the OPERATOR, not to the delimiter word, but
    shlex splits on `<<` and hands back `-EOF`. Read literally that terminator
    never matches, the body never ends, and every command after it is treated as
    data — under-checking a whole file from one heredoc. The dash also changes
    the terminator: bash strips leading TABS from it before comparing.
    """
    opened = []
    arithmetic = 0
    index = 0
    while index < len(words):
        # `(( ... ))` is arithmetic, where `<<` is a left shift. A heredoc
        # opened there never closes and swallows the rest of the block as data.
        if (
            index + 1 < len(words)
            and not words[index].quoted
            and words[index].value in ("(", ")")
            and words[index + 1].value == words[index].value
            and not words[index + 1].space_before
        ):
            arithmetic += 1 if words[index].value == "(" else -1
            arithmetic = max(arithmetic, 0)
            index += 2
            continue
        if (
            arithmetic
            or words[index].value != "<<"
            or words[index].quoted
            or index + 1 >= len(words)
        ):
            index += 1
            continue
        word = words[index + 1]
        if not word.value.startswith("-") or word.space_before:
            # A dash SEPARATED from `<<` is part of the delimiter word, not the
            # operator: `cat << -EOF` ends on a line saying `-EOF` and strips no
            # tabs. shlex tokenises it identically to `<<-EOF`, so only the
            # adjacency tells them apart.
            opened.append((word.value, False))
            index += 2
        elif word.value != "-":
            opened.append((word.value[1:], True))
            index += 2
        elif index + 2 < len(words):
            # `<<- EOF`: bash allows whitespace between the operator and the
            # word, so the dash arrives on its own and the delimiter is the word
            # after it. Taking the dash as the delimiter yields an empty one,
            # which closes the body on the first blank line.
            opened.append((words[index + 2].value, True))
            index += 3
        else:
            index += 2
    return opened


# A line that is nothing but a comment. Joined into a body it would swallow
# everything after it, since a comment runs to the end of the text it is in.
COMMENT_ONLY = re.compile(r"^\s*#")

# What a newline becomes when a line is joined into an unclosed bracket. Inside
# a `{ … }` body or a `( … )` subshell it separates COMMANDS; inside an array
# it separates WORDS, since an array holds words and not commands.
BODY_SEPARATOR = " ; "
ARRAY_SEPARATOR = " "


def open_bracket(text):
    """How to join the next line into an unclosed bracket, or None if there is none.

    A function body, an array, a subshell and a group each span as many lines
    as they need, and a line inside one is not a command of its own. WHICH
    bracket is open decides how the lines join: an array holds words, and
    everything else holds commands.

    Read from the lexer's words, so a bracket inside quotes is an ordinary
    character; and a `)` that closes nothing is ignored, because that is what a
    case ARM's looks like.
    """
    try:
        words = lex_words(text)
    except ValueError:
        return None
    open_kinds = []
    for index, word in enumerate(words):
        if word.quoted:
            continue
        if word.value == "(":
            open_kinds.append(
                ARRAY_SEPARATOR if assigned_array(words, index) is not None
                else BODY_SEPARATOR
            )
        elif word.value == "{":
            open_kinds.append(BODY_SEPARATOR)
        elif word.value in (")", "}") and open_kinds:
            open_kinds.pop()
    return open_kinds[-1] if open_kinds else None


def heredocs_opened_by(command):
    """The heredocs one command opens, or none if it cannot be tokenised."""
    masked, _ = mask_expressions(command)
    try:
        return heredocs_opened(lex_words(masked))
    except ValueError:
        return []


def shell_commands(text):
    r"""Every complete command in a block of shell, and where it starts.

    Yields `(offset, text, in_heredoc)`, the offset counting PHYSICAL lines
    from the start of the block. A command is not a line: a backslash
    continuation and a quote left open each carry it onto the next one, and a
    heredoc BODY is data the shell runs none of.

    Both readers of a block ask this — the one checking literal apt commands
    and the one following `${{ }}` references — because asking it in two places
    is how an expression on the far side of an `echo \` came to be read as a
    command of its own, and a workflow that never runs apt was rejected for a
    package inside a variable it only echoes.
    """
    heredocs = []
    pending = None
    pending_offset = 0
    separator = ""

    for offset, raw in enumerate(text.splitlines()):
        if heredocs:
            # A heredoc body is matched against PHYSICAL lines: the shell joins
            # no continuations inside one, so a body line ending in `\` must not
            # swallow the terminator on the next.
            delimiter, tabs_stripped = heredocs[0]
            if (raw.lstrip("\t") if tabs_stripped else raw) == delimiter:
                heredocs.pop(0)
            else:
                yield offset, raw, True
            continue

        if pending is None:
            pending_offset = offset
            pending = raw
            separator = ""
        elif separator in (BODY_SEPARATOR, ARRAY_SEPARATOR) and COMMENT_ONLY.match(raw):
            # Inside a body the lines are joined, and a comment runs to the end
            # of what it is joined into — so a line that is only a comment is
            # left out rather than allowed to swallow the rest of the body.
            continue
        else:
            # A backslash-newline pair is removed ENTIRELY, so `cma\` followed
            # by `ke` is the one word `cmake`; the conventional layout keeps its
            # separator either way, being the whitespace already before the
            # backslash or the next line's indentation. An open QUOTE is the
            # other case: there the newline is part of the word and is kept.
            pending = f"{pending}{separator}{raw}"

        # A backslash ending a COMMENT continues nothing: bash discarded the
        # rest of that line before ever seeing it, and the command beneath runs
        # on its own. Joining anyway makes that command part of the comment and
        # it vanishes without even a notice.
        if continues_the_line(pending) and not holds_a_comment(pending[:-1]):
            pending = pending[:-1]
            separator = ""
            continue

        # ...but not a quote inside a COMMENT: bash discarded the rest of that
        # line before ever seeing the apostrophe in `# don't`, so carrying it
        # would swallow the command beneath into the comment.
        if not holds_a_comment(pending) and ends_inside_a_quote(pending):
            separator = "\n"
            continue

        # A body written across lines is still one command — `unused() {` and
        # the lines under it are a declaration, not commands of their own. The
        # lines are joined with a SEPARATOR rather than a space, because a
        # newline separates commands inside a body and this lexer reads it as
        # whitespace: joined with a space, `echo hi` would become two more
        # words of the install beneath it.
        joining = open_bracket(pending)
        if joining is not None:
            separator = joining
            continue

        command, pending = pending, None
        yield pending_offset, command, False
        # Looked for on EVERY command, not only the ones naming apt: the line
        # that opens a heredoc is usually `cat > file <<EOF`, which names
        # nothing. The body starts after the command's LAST physical line.
        heredocs = heredocs_opened_by(command)

    if pending is not None:
        # A trailing `\` at the end of the block: still a command, and the last
        # one, so whatever heredoc it opens has no body to open.
        yield pending_offset, pending, False


def scan_shell(path, text, first_line, exact_lines):
    """Report the packages installed by a block of shell.

    `exact_lines` says whether a physical line of `text` maps to a file line —
    true for a literal block or a plain scalar, false once YAML has folded the
    block, where every finding is reported against the line the block starts on.
    """
    defined = {}
    for offset, command, in_heredoc in shell_commands(text):
        line = first_line + offset if exact_lines else first_line

        if in_heredoc:
            if "apt" in command and "install" in command:
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
            words = lex_words(masked)
        except Unlexable as error:
            # Only worth reporting for a line that could hold an install; every
            # other unbalanced quote in a workflow is somebody else's business.
            # Asked of the words read BEFORE the quote where there are any,
            # since the raw text does not spell `a\pt-get` as apt.
            if (
                any(is_apt(word.value) for word in error.words)
                or ("apt" in command and "install" in command)
            ):
                emit("NOTICE", path, line, f"could not be tokenised ({error}), not checked: {command.strip()}")
            continue
        if (
            any(is_apt(word.value) or is_partly_assembled(word.value) for word in words)
            or runs_a_script(words)
            or touches_a_definition(words, defined)
        ):
            # Asked of the TOKENS, not the raw line: `a\pt-get` is `apt-get`
            # once the escape is gone, and a substring test on the source text
            # skips it without checking a package or saying a word. A word part
            # literal and part expression may be an apt invocation too, and is
            # announced rather than passed over.
            scan_command(path, line, words, expressions, defined)


def scalar_values(node, path=()):
    """Every scalar in a workflow, paired with the key PATH it sits under.

    The whole path, not just the nearest key, because a referenced name is
    scoped to its context: `${{ env.command }}` names the `command` under
    `env:`, not an action input that happens to be called `command` too. A
    sequence passes its own path down, so the entries of `matrix.include` are
    still the matrix's.
    """
    if isinstance(node, yaml.MappingNode):
        for name, value in node.value:
            inner = path + ((name.value,) if isinstance(name, yaml.ScalarNode) else ((),))
            if isinstance(value, yaml.ScalarNode):
                yield inner, value
            else:
                yield from scalar_values(value, inner)
    elif isinstance(node, yaml.SequenceNode):
        for position, child in enumerate(node.value):
            # The INDEX is part of the path: two steps in one job each set
            # `env.command`, and without it their values are indistinguishable.
            if isinstance(child, yaml.ScalarNode):
                yield path + (position,), child
            else:
                yield from scalar_values(child, path + (position,))


# The contexts whose values can hold a command a `run:` then executes. `github`
# and the rest are metadata: following `github.event.repository.name` would put
# `name` in scope and drag every step's name back in.
COMMAND_CONTEXTS = ("matrix", "env", "inputs", "vars")

# `matrix.extra_deps`, `matrix['extra_deps']` and `matrix.include.extra_deps`
# all select a value; an expression may hold several, as in
# `${{ matrix.primary || matrix.fallback }}`.
CONTEXT_REFERENCE = re.compile(
    r"\b(" + "|".join(COMMAND_CONTEXTS) + r")\b"
    r"((?:\s*\.\s*[A-Za-z_][A-Za-z0-9_-]*|\s*\[\s*'[^']*'\s*\]|\s*\[\s*\"[^\"]*\"\s*\])+)"
)
PROPERTY_ACCESS = re.compile(
    r"""\.\s*([A-Za-z_][A-Za-z0-9_-]*)|\[\s*'([^']*)'\s*\]|\[\s*"([^"]*)"\s*\]"""
)


# `${{ 'env.command' }}` is a string LITERAL and dereferences nothing, while the
# quotes in `matrix['extra_deps']` are an INDEX and name a value. The index form
# is matched first and kept; whatever else is quoted is blanked before any
# reference is looked for.
EXPRESSION_STRING = re.compile(
    r"""(\[\s*(['"])[^'"]*\2\s*\])|('[^']*'|"[^"]*")"""
)


def without_literals(text):
    """The expression with its string literals blanked, indexes left alone."""
    return EXPRESSION_STRING.sub(lambda found: found.group(1) or "''", text)


def job_scope(path):
    """The job a path belongs to, as a prefix, or () for the workflow itself."""
    if len(path) >= 2 and path[0] == "jobs":
        return path[:2]
    return ()


def referenced_values(expression):
    """Every (context, access chain) a `${{ ... }}` selects.

    EVERY one, because `${{ matrix.primary || matrix.fallback }}` may execute
    either. The WHOLE chain, not its leaf: `matrix.target.command` names the
    `command` of `target`, and collapsing it to `command` matched every value
    in the job spelled that way — including a field of another dimension that
    nothing runs.
    """
    found = set()
    for context, path in CONTEXT_REFERENCE.findall(without_literals(expression)):
        # One access, `.name` or `['name']`, matches one of three groups, so
        # the name is whichever of them is not empty.
        chain = tuple(
            next(part for part in access if part)
            for access in PROPERTY_ACCESS.findall(path)
            if any(access)
        )
        if chain:
            found.add((context, chain))
    return found


# `include:` is the matrix's own keyword rather than a dimension: an entry under
# it contributes its keys as names of their own, so `${{ matrix.extra_deps }}`
# reaches `matrix/include/0/extra_deps` without naming `include` at all.
#
# `exclude:` is NOT here on purpose. Its entries name combinations to DROP, so
# nothing under one is a value the workflow ever runs, and stepping over the
# keyword made a scalar written there resolve as though it were.
MATRIX_KEYWORDS = {"include"}


def excluded_outright(scope, values):
    """The (dimension, value) pairs `exclude:` removes from every combination.

    An `exclude:` entry names a COMBINATION, so a value it mentions still runs
    wherever the rest of the entry does not match — unless the entry names that
    one dimension and nothing else, which leaves no combination holding it.

    Anything finer is combination algebra this file has no business holding:
    two entries between them may cover every value of another dimension and so
    exclude a value outright as well. The direction of being wrong is a value
    read that some combination still runs, which is a package checked rather
    than a package missed.
    """
    entries = {}
    for path, value in values:
        if "matrix" not in path or path[:len(scope)] != scope:
            continue
        matrix = path.index("matrix")
        if len(path) == matrix + 4 and path[matrix + 1] == "exclude":
            entries.setdefault(path[:matrix + 3], []).append((path[-1], value.value))
    return {members[0] for members in entries.values() if len(members) == 1}


def selects_the_value(path, chain):
    """Whether this matrix scalar is what an access chain names.

    The chain's names appear IN ORDER after `matrix`, separated only by
    sequence indices and the matrix's own keywords: `matrix.command` reaches
    `matrix/command/0`, and `matrix.target.command` reaches
    `matrix/target/0/command` — but not `matrix/metadata/0/command`, which
    another dimension only happens to spell the same way.
    """
    remainder = list(path[path.index("matrix") + 1:])
    for name in chain:
        while remainder and remainder[0] != name and (
            isinstance(remainder[0], int) or remainder[0] in MATRIX_KEYWORDS
        ):
            remainder.pop(0)
        if not remainder or remainder[0] != name:
            return False
        remainder.pop(0)
    # Whatever is left has to be the ENTRIES of what was named. A key left over
    # means the chain named an object rather than a value: `${{ matrix.target }}`
    # interpolates the object itself and runs no install.
    return all(isinstance(part, int) for part in remainder)


def resolve_reference(run_path, context, chain, values):
    """The scalar(s) a reference selects from where the `run:` stands.

    A reference names a value IN SCOPE, and the nearest definition wins: two
    steps in one job may each set `env.command`, and a `run:` sees its own.
    A matrix is the exception — every `include:` entry runs, so all of them
    count.
    """
    scope = job_scope(run_path)
    name = chain[-1]
    if context == "matrix":
        excluded = excluded_outright(scope, values)
        return [
            value for path, value in values
            if "matrix" in path and path[:len(scope)] == scope
            and selects_the_value(path, chain)
            and (name, value.value) not in excluded
        ]
    if context == "env":
        # env is INHERITED, not shared: a step sees its own, then its job's,
        # then the workflow's, and never another STEP's. So the block it is
        # written in has to CONTAIN this `run:` — ranking by shared path alone
        # picks a sibling step, which shares `jobs/<job>/steps` with it.
        candidates = []
        for path, value in values:
            if path[-1] != name or "env" not in path:
                continue
            container = path[:path.index("env")]
            if container == run_path[:len(container)]:
                candidates.append((container, value))
        if not candidates:
            return []
        # The innermost block that contains it wins.
        return [max(candidates, key=lambda found: len(found[0]))[1]]
    if context == "inputs":
        # A workflow input's value lives under `default:`, one level below the
        # name the expression uses — and under the workflow's own DECLARATION
        # of it. A matrix dimension may be called `inputs` too, and matching
        # every path holding the word resolved one of its fields for an
        # expression that reaches only `on:`.
        return [
            value for path, value in values
            if len(path) >= 4 and path[0] == "on"
            and path[1] in ("workflow_call", "workflow_dispatch")
            and path[2] == "inputs"
            and (path[-1] == name
                 or (path[-1] == "default" and path[-2] == name))
        ]
    # `vars` are repository settings; nothing in the file to scan.
    return []


def literal_alternatives(expression):
    """The string LITERALS an expression may evaluate to.

    `${{ env.COMMAND || 'apt-get install -y cmake' }}` runs the literal when
    the value is empty, so the literal is a command like any other. An index
    is not an alternative — `matrix['pkg']` names a value — and neither is the
    template of a function call, where `format('… {0}', x)` holds no command
    until it is assembled; that shape is not a plain reference and never
    reaches here.
    """
    return [
        match.group(3)[1:-1]
        for match in EXPRESSION_STRING.finditer(expression)
        if match.group(3)
    ]


# A name, or a chain of them: `matrix`, `github.ref`, `steps.build.outputs.x`.
# Blanked before the operators are, so that what remains of a CHOICE is
# nothing — while a function call leaves its brackets and commas behind.
IDENTIFIER_CHAIN = re.compile(
    r"[A-Za-z_][A-Za-z0-9_-]*(?:\s*\.\s*[A-Za-z_][A-Za-z0-9_-]*)*"
)

# The operators an expression may CHOOSE with: alternation, and the
# comparisons that decide which alternative wins.
CHOICE_OPERATORS = re.compile(r"\|\||&&|==|!=|>=|<=|[<>!]|''|\s+")


def chooses_a_command(expression):
    """Whether this expression CHOOSES among the things written in it.

    `${{ matrix.extra_deps }}`, `${{ a || b }}` and
    `${{ ref == 'main' && 'apt-get …' || 'true' }}` each evaluate to one of
    their parts, so every part is something the workflow may run.
    `${{ format('sudo apt-get install -y {0}', matrix.package) }}` ASSEMBLES
    one instead: the `run:` text holds no apt invocation and the matrix value
    is a bare word, so checking either of them checks nothing.

    APPROXIMATION: the operand of a comparison is read as an alternative too,
    since telling `a == 'x' && 'cmd'` apart from `a || 'cmd'` needs the
    expression's grammar rather than its vocabulary. A comparison operand is
    not a command, but reading one as a command finds no apt in it.
    """
    # Every NAME, not only the ones that can carry a command: `github.ref` is
    # not a command context and still belongs to the choice being made.
    remainder = IDENTIFIER_CHAIN.sub("", without_literals(expression)[3:-2])
    return CHOICE_OPERATORS.sub("", remainder) == ""


def command_segments(line):
    """The line's WORDS, split at its unquoted separators.

    Words rather than strings: rebuilding a segment by joining values throws
    away the quoting, and a name holding a `;` would split the segment that
    holds it. Built on the lexer so a `;` inside quotes stays part of its word.

    A `[[ … ]]` test is ONE command however many `&&` it holds: those are the
    conditional's operators, not the shell's separators, and splitting there
    left `"${{ github.ref }}" == refs/tags/*` looking like a command of its
    own with the expression at the front of it.
    """
    try:
        words = lex_words(line)
    except ValueError:
        return []
    segments = [[]]
    conditional = False
    for word in words:
        if not word.quoted and word.value in ("[[", "(("):
            conditional = True
        elif not word.quoted and word.value in ("]]", "))"):
            conditional = False
        if word.value in SEPARATORS and not word.quoted and not conditional:
            segments.append([])
        else:
            segments[-1].append(word)
    return [segment for segment in segments if segment]


def is_a_step_script(path):
    """Whether this scalar is a step's `run:`, and so a script GitHub executes.

    `env: {run: ...}` is a variable it exports, `outputs: {run: ...}` is a value
    it publishes, and `with: {run: ...}` is an input it passes on. None of them
    are executed, and all of them end in a key called `run`.
    """
    # The WHOLE path, not its last three keys: a matrix dimension may be called
    # `steps` and hold objects with a `run:` in them, and that suffix is
    # indistinguishable from a real step's.
    return (
        len(path) == 5
        and path[0] == "jobs"
        and path[2] == "steps"
        and isinstance(path[3], int)
        and path[4] == "run"
    )


def command_word(words):
    """The word that supplies this segment's command, or None if there is none.

    Assignments, redirections, reserved words and invocation prefixes come
    BEFORE a command without being one: `FLAG=1 cmd` runs cmd with a variable
    set for it, `if cmd; then` runs it as a condition, `sudo cmd` runs it as
    root. The first word that is none of those is the command.

    LIMITATION: only the OPTIONS of an invocation prefix are stepped over, not
    the arguments they take, so `sudo -u root cmd` reads `root` as the command.
    Naming which of sudo's options take one is a table this file has no
    business holding; no workflow uses that form, and the direction of being
    wrong is a value unread rather than a package invented.
    """
    # A case ARM's pattern is syntax; bash runs the list after it.
    pattern_end = case_pattern_end(words)
    if pattern_end is not None:
        words = words[pattern_end + 1:]

    index = 0
    saw_invocation = False
    while index < len(words):
        word = words[index]
        token = word.value
        if not word.quoted and token in CONTROL_PREFIXES:
            index += 1
            continue
        if token in INVOCATION_PREFIXES:
            # No quoting guard, unlike the reserved words above: `'sudo' cmd`
            # runs sudo. Quotes take a RESERVED WORD's meaning away and leave a
            # command name exactly as it was.
            saw_invocation = True
            index += 1
            continue
        if saw_invocation and token.startswith("-") and not word.quoted:
            # An option belongs to the prefix that takes it — `sudo -E` — and
            # only to a prefix: a bare `-x` starts no command, so reading one
            # as prefix material would follow a value nothing runs.
            index += 1
            continue
        if token in REDIRECTIONS and not word.quoted:
            index += 2
            continue
        if is_an_assignment(word):
            index += 1
            assignment = assigned_array(words, index)
            if assignment is not None:
                # `deps=(a b c)` assigns an ARRAY, and the parenthesis is not
                # the subshell a bare `(` would be. Nothing here RUNS: the
                # elements become a command only where `"${deps[@]}"` puts them
                # at one, which is where they are read.
                _, _, index = assignment
            continue
        if token.isdigit() and index + 1 < len(words) and words[index + 1].value in REDIRECTIONS:
            index += 1
            continue
        return word
    return None


def written_before_the_command(words):
    """Whether this segment holds a command of its own, before any expression.

    Assignments, redirections, reserved words and invocation prefixes come
    BEFORE a command without being one: `FLAG=1 ${{ env.command }}` runs the
    value with a variable set for it, `if ${{ env.command }}; then` runs it as
    a condition and `sudo ${{ env.command }}` runs it as root — in each the
    expression still supplies the command. Anything else written here means the
    command is this segment's and the expression is data in it.

    LIMITATION: only the OPTIONS of an invocation prefix are stepped over, not
    the arguments they take, so `sudo -u root ${{ env.command }}` reads as a
    command written here and its value is not followed. Naming which of sudo's
    options take one is a table this file has no business holding; no workflow
    uses that form, and the direction of being wrong is a value unread rather
    than a package invented.
    """
    word = command_word(words)
    if word is None:
        # Nothing but prefixes: assignments alone ARE the command — `OUT=x.apk`
        # sets a variable and runs nothing else — so the segment is written
        # here.
        return True
    # Quoting does not decide this, POSITION does. GitHub substitutes inside
    # the quotes, so `"${{ env.COMMAND }}" install -y x` runs what the value
    # names; and `[[ "${{ github.ref }}" == refs/tags/* ]]` is a test because
    # `[[` is the command there, not because the expression is quoted.
    return not WHOLE_EXPRESSION.match(word.value)


def scalar_line(value, offset=0):
    """The file line a scalar's Nth line sits on.

    A plain scalar sits ON its line; a literal block keeps its lines, so they
    follow the indicator; a folded block no longer has a line-for-line mapping,
    so everything in it is reported against the block itself.
    """
    line = value.start_mark.line + 1
    if value.style == "|":
        return line + 1 + offset
    if value.style is None or value.style == "":
        return line + offset
    return line


def scan_scalar(path, value):
    """Scan one YAML scalar as shell, honouring how its block was written."""
    line = value.start_mark.line + 1
    # A plain scalar sits ON its line; a literal block keeps its lines, so they
    # follow the indicator; a folded block no longer has a line-for-line
    # mapping, so its findings are reported against the block itself.
    if value.style is None or value.style == "":
        scan_shell(path, value.value, line, exact_lines=True)
    elif value.style == "|":
        scan_shell(path, value.value, line + 1, exact_lines=True)
    else:
        scan_shell(path, value.value, line, exact_lines=False)


def as_written(word, expressions):
    """One word, back as shell text that lexes to exactly this word again.

    An operator is written as it stands — quoting a `>` would turn a
    redirection into a package name — and everything else goes through
    shlex.quote, which is what carries a name holding a space or a bracket
    across the trip intact.
    """
    text = unmask(word.value, expressions)
    if not word.quoted and word.value in WORD_BOUNDARIES:
        return text
    return shlex.quote(text)


def assigned_array(words, index):
    """The array `NAME=( … )` whose bracket is at `index`, or None.

    Returns the name, the words inside, and the index after the group. The
    bracket has to be written hard against a bare `NAME=` or `NAME+=`: a spaced
    `(` is a subshell, and a word that merely holds an `=` is not an array
    prefix — `RESULT=$(` ends in a dollar and RUNS what the bracket opens.
    """
    if (
        index < 1
        or index >= len(words)
        or words[index].value != "("
        or words[index].quoted
        or words[index].space_before
        or not ARRAY_PREFIX.match(words[index - 1].value)
    ):
        return None
    inside, after = walk_to_closing_paren(words, index)
    return words[index - 1].value, inside, after


def rebased(words, expressions, into):
    """Remembered words, renumbered into another command's expression list.

    An array is assigned in one command and run in another, and each command
    masks its own expressions from zero — so a sentinel a remembered word
    carries means nothing where the array is expanded until it is moved across.
    """
    base = len(into)
    into.extend(expressions)
    return [
        Word(
            re.sub(EXPRESSION_PREFIX + r"(\d+)__",
                   lambda match: EXPRESSION_SENTINEL.format(int(match.group(1)) + base),
                   word.value),
            word.literal_dollar, word.space_before, word.literal_backtick,
            word.quoted, word.literal_brace,
        )
        for word in words
    ]


def remembered(token, defined):
    """The key `defined` holds for this word, or None: an array, a variable or
    a function, whichever way the word names one."""
    expansion = ARRAY_EXPANSION.match(token)
    if expansion and ("array", expansion.group(1)) in defined:
        return "array", expansion.group(1)
    variable = SHELL_VARIABLE.match(token)
    if variable:
        name = variable.group(1) or variable.group(2)
        if ("variable", name) in defined:
            return "variable", name
    if ("function", token) in defined:
        return "function", token
    return None


def held(name, kind, defined, expressions):
    """What is remembered under this name, renumbered into `expressions`."""
    remembered_words = defined.get((kind, name))
    return rebased(*remembered_words, expressions) if remembered_words else []


def remember_array(prefix, inside, defined, expressions):
    """Remember an array's elements, appending when the prefix says `+=`."""
    name = prefix.rstrip("=").rstrip("+")
    before = held(name, "array", defined, expressions) if prefix.endswith("+=") else []
    defined[("array", name)] = (before + inside, expressions)


def remember(assignments, defined, expressions):
    """Remember what a command's assignments set, when nothing follows them.

    `COMMAND='apt-get install -y x'` on its own sets the variable for the rest
    of the script; `FLAG=1 cmd` sets it for cmd alone and is gone after. The
    value becomes WORDS, because that is what `$COMMAND` expands to.

    `NAME+=` APPENDS: a script that builds its command in pieces —
    `COMMAND='apt-get install -y '` then `COMMAND+=pkg` — installs pkg, and
    replacing the value would leave a command that is not apt.
    """
    if defined is None:
        return
    for word in assignments:
        name, _, value = word.value.partition("=")
        appending = name.endswith("+")
        name = name.rstrip("+")
        before = held(name, "variable", defined, expressions) if appending else []
        defined[("variable", name)] = (
            before + [Word(part, quoted=True, literal_dollar=True, literal_backtick=True,
                           literal_brace=True)
                      for part in value.split()],
            expressions,
        )


def defined_function(words, index):
    """The function `NAME () { … }` beginning at `index`, or None.

    Returns the name, the words of its body, and the index after it. Declaring
    a function RUNS none of it, so the body is remembered rather than read
    where it is written — the same rule an array's elements follow.

    The brackets have to be next to each other: `foo (cmd)` is the command foo
    followed by a subshell, while `foo ()`, `foo()` and `foo ( )` all declare.
    """
    if index + 3 >= len(words):
        return None
    opener, closer, body = words[index + 1], words[index + 2], words[index + 3]
    if (
        opener.value != "(" or closer.value != ")" or body.value != "{"
        or opener.quoted or closer.quoted or body.quoted
    ):
        return None
    depth = 0
    for position in range(index + 3, len(words)):
        if words[position].quoted:
            continue
        if words[position].value == "{":
            depth += 1
        elif words[position].value == "}":
            depth -= 1
            if not depth:
                return words[index].value, words[index + 4:position], position + 1
    return words[index].value, words[index + 4:], len(words)


def touches_a_definition(words, defined):
    """Whether these words define something, or run something already defined.

    Asked because a definition and its use are separate commands and neither
    need hold apt: `deps=( … )` has to be remembered when it does not, and
    `"${deps[@]}"` names no command of its own for the line to be read for.
    """
    for index, word in enumerate(words):
        if assigned_array(words, index) is not None or defined_function(words, index):
            return True
        if is_an_assignment(word):
            return True
        expansion = ARRAY_EXPANSION.match(word.value)
        variable = SHELL_VARIABLE.match(word.value)
        if expansion and ("array", expansion.group(1)) in defined:
            return True
        if variable and ("variable", variable.group(1) or variable.group(2)) in defined:
            return True
        if ("function", word.value) in defined:
            return True
    return False


def as_words(text):
    """A variable's value, written so it lexes to the words bash makes of it.

    Bash splits the value on whitespace and stops: the metacharacters inside
    are ordinary characters rather than operators, and nothing in it is
    expanded a second time. Quoting each word on its own says exactly that —
    `echo ok ; apt-get …` runs echo with `;` as an argument and never reaches
    apt, so reading the value as fresh shell rejects a workflow for a command
    that cannot run.

    A `${{ }}` expression is the opposite case and keeps its text: GitHub
    substitutes into the script before bash parses any of it, so a separator
    written there really does separate.
    """
    return " ".join(shlex.quote(word) for word in text.split())


def script_argument(words):
    """The word run as a SCRIPT, or None if nothing hands one over.

    GitHub substitutes into the `run:` text before bash sees any of it, so an
    expression written there IS the script — quoted or not, because the quotes
    are the shell's and the value lands inside them.

    Quoting the SHELL's own name changes nothing either: `'bash' -c '…'` runs
    bash. That is what separates a command name from a reserved word, where
    the quotes do matter — bash looks for a command called `if` and finds none.
    """
    word = command_word(words)
    if word is None:
        return None
    name = word.value.rsplit("/", 1)[-1]
    if name == "eval":
        # `eval` takes its script as OPERANDS: it joins them all into one
        # string and executes that, so `eval 'apt-get install -y' pkg` runs
        # both words. The quotes are gone by now, and joining the values with
        # a space is what bash is left holding.
        operands = words[words.index(word) + 1:command_ends(words, words.index(word))]
        if not operands:
            return None
        return Word(" ".join(operand.value for operand in operands))
    if name not in SHELL_COMMANDS:
        return None
    saw_argument = False
    for index, candidate in enumerate(words[words.index(word) + 1:], words.index(word) + 1):
        if hands_over_a_script(candidate) and index + 1 < len(words):
            return words[index + 1]
        if saw_argument:
            saw_argument = False
        elif candidate.value in SHELL_OPTIONS_WITH_ARGUMENT and not candidate.quoted:
            saw_argument = True
        elif not candidate.value.startswith("-"):
            # The first operand is the script FILE, and bash reads no more
            # options after it: `bash build.sh -c x` hands `-c` to build.sh.
            return None
    return None


def visit_reference(path, referenced, run_path, values, scanned, followed,
                    line=None, arguments="", split=False):
    """Scan a value a reference reached, and follow whatever IT names.

    `arguments` is what the call site wrote AFTER the reference. A value may
    supply only part of a command — `env.COMMAND: apt-get install -y` with
    `run: ${{ env.COMMAND }} pkg` — and neither half holds a package on its
    own, so the two are read together or the install is never seen.
    """
    text = as_words(referenced.value) if split else referenced.value
    if arguments or split:
        if (id(referenced), arguments, split) not in scanned:
            scanned.add((id(referenced), arguments, split))
            scan_shell(path, " ".join(part for part in (text, arguments) if part),
                       line, exact_lines=False)
    elif id(referenced) not in scanned:
        scanned.add(id(referenced))
        scan_scalar(path, referenced)
    if (id(referenced), arguments) not in followed:
        followed.add((id(referenced), arguments))
        # The arguments travel WITH the chain: `${{ env.COMMAND }} pkg` where
        # `env.COMMAND` is itself `${{ matrix.base }}` installs pkg, so the
        # words written at the call site belong to whatever the chain ends at.
        follow_references(path, referenced, run_path, values, scanned, followed,
                          arguments)


def follow_references(path, value, run_path, values, scanned, followed, arguments=""):
    """Scan the values this scalar's expressions name, and the values THOSE name.

    A `run:` may reach its command through more than one hop — a step writing
    `env.COMMAND: ${{ matrix.install }}` and running `${{ env.COMMAND }}` —
    and stopping at the first hop scanned an env value that is itself only an
    expression, leaving the matrix entry that holds the install neither checked
    nor announced.

    Resolution stays anchored to the `run:`, because that is where GitHub
    evaluates the expression; `followed` is what stops two values that name
    each other from chasing one another for ever.
    """
    arrays = {}
    for offset, command, in_heredoc in shell_commands(value.value):
        if in_heredoc:
            continue
        # MASKED before segmenting: `${{ a || b }}` holds a `||` that is part
        # of the expression, not a shell separator, and splitting the raw text
        # tears the expression in half.
        masked, all_expressions = mask_expressions(command)
        for segment in command_segments(masked):
            # An array's elements are remembered where they are assigned and
            # read where `"${deps[@]}"` puts them at a command position —
            # the same rule the literal scan follows, because an initializer
            # that is only printed runs nothing.
            assignment = next(
                (found for found in map(lambda at: assigned_array(segment, at),
                                        range(len(segment)))
                 if found is not None),
                None,
            )
            if assignment is not None:
                # Remembered, and the segment read on: an assignment can be a
                # PREFIX — `deps=(x) cmd` sets it for that one command and runs
                # cmd — so what follows the group is still a command.
                remember_array(assignment[0], assignment[1], arrays, all_expressions)

            word = command_word(segment)
            expanded = ARRAY_EXPANSION.match(word.value) if word is not None else None
            if expanded is not None and ("array", expanded.group(1)) in arrays:
                elements = held(expanded.group(1), "array", arrays, all_expressions)
                segment = elements + segment[segment.index(word) + 1:]

            found = [
                (position, all_expressions[int(number)])
                for position, word in enumerate(segment)
                for number in re.findall(EXPRESSION_PREFIX + r"(\d+)__", word.value)
            ]
            # `$COMMAND` at a command position runs whatever the variable
            # holds, bash splitting it into words. When the workflow's own
            # `env:` sets it, that value is in this file and is read like any
            # other reference; when it is set by the script, or by the runner,
            # it is ordinary shell and there is nothing here to read.
            word = command_word(segment)
            if word is not None and not word.quoted:
                variable = SHELL_VARIABLE.match(word.value)
                if variable:
                    name = variable.group(1) or variable.group(2)
                    # With the words after it, for the same reason an
                    # expression is read with them: `$COMMAND pkg` splits the
                    # variable and passes the rest along.
                    suffix = segment[segment.index(word) + 1:]
                    written = " ".join(as_written(after, all_expressions) for after in suffix)
                    carried = " ".join(part for part in (written, arguments) if part)
                    for referenced in resolve_reference(run_path, "env", (name,), values):
                        visit_reference(path, referenced, run_path, values, scanned,
                                        followed, scalar_line(value, offset), carried,
                                        split=True)

            # Whether anything of the command is written HERE.
            # `echo ${{ github.ref }}` is an ordinary command with a value
            # interpolated into it, and its apt is read as usual; a segment
            # that is nothing but an expression has no command here at all.
            #
            # `bash -c '${{ env.SCRIPT }}'` is the other way a value becomes a
            # command: the segment's command IS written here, and the value is
            # executed all the same, as the script the shell is handed.
            if written_before_the_command(segment):
                script = script_argument(segment)
                if script is None:
                    continue
                # A variable is a script as readily as an expression is, and
                # the quotes around `bash -c "$SCRIPT"` do not stop it being
                # one: the value is what bash reads, not an argument to it.
                variable = SHELL_VARIABLE.match(script.value)
                if variable:
                    name = variable.group(1) or variable.group(2)
                    for referenced in resolve_reference(run_path, "env", (name,), values):
                        visit_reference(path, referenced, run_path, values, scanned,
                                        followed, scalar_line(value, offset), arguments)
                if not WHOLE_EXPRESSION.match(script.value):
                    continue
                supplying = [
                    (all_expressions[int(number)], [], False)
                    for number in re.findall(EXPRESSION_PREFIX + r"(\d+)__", script.value)
                ]
            else:
                supplying = [(expression, segment[position + 1:], segment[position].quoted)
                             for position, expression in found]

            for expression, suffix, was_quoted in supplying:
                resolved = 0
                written = " ".join(as_written(word, all_expressions) for word in suffix)
                carried = " ".join(part for part in (written, arguments) if part)
                for context, chain in referenced_values(expression):
                    for referenced in resolve_reference(run_path, context, chain, values):
                        resolved += 1
                        if was_quoted and len(referenced.value.split()) != 1:
                            # Quoted, the whole value is ONE word: bash looks
                            # for a command with that name and finds none, so
                            # nothing runs and its packages are not installed.
                            continue
                        visit_reference(path, referenced, run_path, values, scanned,
                                        followed, scalar_line(value, offset), carried)
                if not chooses_a_command(expression):
                    # The whole command is built by the expression, so it
                    # exists nowhere in the file to be checked. Announced,
                    # never silent.
                    emit("NOTICE", path, scalar_line(value, offset),
                         f"builds its command with an expression, assembled at run time "
                         f"and not checked: {expression}")
                    continue

                # `a || 'literal'` runs the literal when the value is empty, so
                # the literal is one of the commands this may be.
                for literal in literal_alternatives(expression):
                    resolved += 1
                    scan_shell(path, " ".join(part for part in (literal, carried) if part),
                               scalar_line(value, offset), exact_lines=False)

                if not resolved:
                    # `${{ vars.COMMAND }}` is a repository setting, and a
                    # required input has no default: the command exists, but
                    # not in this file. Resolving to nothing and saying nothing
                    # reads exactly like a clean result.
                    emit("NOTICE", path, scalar_line(value, offset),
                         f"names a command this file does not hold, so it was "
                         f"not checked: {expression}")


def scan_workflow(path, text):
    """Scan every scalar a workflow can EXECUTE.

    `run:`, and whatever a `run:` interpolates — resolved from where that
    `run:` stands, so a name reaches the value it actually names.

    LIMITATION: shell reached any other way — an action's `with:` inputs, a
    composite action's own steps — is not scanned. Nothing here uses those; a
    workflow that starts to would need this widened.
    """
    node = yaml.compose(text)
    if node is None:
        return
    values = list(scalar_values(node))
    scanned = set()

    for run_path, value in values:
        # A `run:` is a script when it belongs to a STEP, and only then. Naming
        # the contexts that are NOT scripts is a list that grows one review
        # round at a time — `env`, then `with`, then `outputs` — because a key
        # may be called `run` anywhere. This asks the question the other way
        # round, and the answer stops changing.
        if not is_a_step_script(run_path):
            continue
        if id(value) not in scanned:
            scanned.add(id(value))
            scan_scalar(path, value)
        # Seeded with the `run:` itself — keyed the way `visit_reference`
        # keys it — so a value that names it back cannot start the walk over.
        follow_references(path, value, run_path, values, scanned, {(id(value), "")})


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
