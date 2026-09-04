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
# prefix words and their options.
INVOCATION_PREFIXES = {"sudo", "env"}

APT_COMMANDS = {"apt", "apt-get"}


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

    def read(self, size=-1):
        char = super().read(size)
        if char:
            state = self.lexer.state
            if (
                char in "'\""
                and self.last_char == "$"
                and not self.last_was_escaped
                and state not in ("'", '"')
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
                and state == char
            ):
                # The closing quote of one: the span is `$'...'` inclusive.
                self.dollar_quotes.append((self.dollar_quote_opened[0], self.position))
                self.dollar_quote_opened = None
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

    def __init__(self, error, words, state):
        super().__init__(str(error))
        self.words = words
        self.state = state


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
            raise Unlexable(error, words, lexer.state) from error
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
    parts = []
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
        parts.append(word.value)
    return parts, index


def scan_command(path, line, words, expressions):
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
            state = "idle"
            at_command = True
            end_of_options = False
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
            parts, index = walk_to_closing_paren(words, index)
            if state == "packages":
                emit("NOTICE", path, line,
                     f"names a package through a substitution, not checked: {token}({' '.join(parts)})")
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
                parts, index = walk_to_closing_paren(words, index)
                rendered = token[:-1] + "$(" + " ".join(parts) + ")"
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
        elif token in INVOCATION_PREFIXES or token.startswith("-") or "=" in token:
            # Assignments first: `TAG=${{ inputs.tag }}` sets a variable and
            # names no command, and announcing those flagged two lines of this
            # repository's own workflows.
            pass
        elif is_partly_assembled(token):
            # Announced, never silent: the name of the command being run does
            # not appear in the file.
            emit("NOTICE", path, line,
                 f"builds its command name with an expression, assembled at run time "
                 f"and not checked: {unmask(token, expressions)}")
            at_command = False
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


def scan_shell(path, text, first_line, exact_lines):
    """Report the packages installed by a block of shell.

    `exact_lines` says whether a physical line of `text` maps to a file line —
    true for a literal block or a plain scalar, false once YAML has folded the
    block, where every finding is reported against the line the block starts on.
    """
    heredocs = []
    pending = None
    pending_offset = 0

    def scan_one(command, offset):
        """Scan one complete command; returns the heredocs it opens."""
        line = first_line + offset if exact_lines else first_line
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
            return []
        if any(is_apt(word.value) or is_partly_assembled(word.value) for word in words):
            # Asked of the TOKENS, not the raw line: `a\pt-get` is `apt-get`
            # once the escape is gone, and a substring test on the source text
            # skips it without checking a package or saying a word. A word part
            # literal and part expression may be an apt invocation too, and is
            # announced rather than passed over.
            scan_command(path, line, words, expressions)
        # Looked for on EVERY command, not only the ones naming apt: the line
        # that opens a heredoc is usually `cat > file <<EOF`, which names
        # nothing. The body starts after the command's LAST physical line.
        return heredocs_opened(words)

    for offset, raw in enumerate(text.splitlines()):
        if heredocs:
            # A heredoc body is matched against PHYSICAL lines: the shell joins
            # no continuations inside one, so a body line ending in `\` must not
            # swallow the terminator on the next.
            delimiter, tabs_stripped = heredocs[0]
            if (raw.lstrip("\t") if tabs_stripped else raw) == delimiter:
                heredocs.pop(0)
            elif "apt" in raw and "install" in raw:
                # A heredoc body is data, not commands. Announced anyway, so a
                # misread delimiter cannot hide an install silently.
                emit(
                    "NOTICE",
                    path,
                    first_line + offset if exact_lines else first_line,
                    f"is inside a heredoc, so it is data and not a command — not checked: {raw.strip()}",
                )
            continue

        if pending is None:
            pending_offset = offset
            pending = raw
            separator = ""
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

        command, pending = pending, None
        heredocs = scan_one(command, pending_offset)

    if pending is not None:
        # A trailing `\` at the end of the block: still a command, and the last
        # one, so whatever heredoc it opens has no body to open.
        scan_one(pending, pending_offset)


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
    """Every (context, value name) a `${{ ... }}` selects.

    EVERY one, because `${{ matrix.primary || matrix.fallback }}` may execute
    either. Only the LAST access of each chain is the value — `matrix.include.
    extra_deps` names `extra_deps` — and only within a context that can carry a
    command.
    """
    found = set()
    for context, path in CONTEXT_REFERENCE.findall(without_literals(expression)):
        accesses = PROPERTY_ACCESS.findall(path)
        if accesses:
            name = next((part for part in accesses[-1] if part), None)
            if name:
                found.add((context, name))
    return found


def shared_prefix(first, second):
    """How many leading path elements two scalars have in common."""
    length = 0
    for one, other in zip(first, second):
        if one != other:
            break
        length += 1
    return length


def resolve_reference(run_path, context, name, values):
    """The scalar(s) a reference selects from where the `run:` stands.

    A reference names a value IN SCOPE, and the nearest definition wins: two
    steps in one job may each set `env.command`, and a `run:` sees its own.
    A matrix is the exception — every `include:` entry runs, so all of them
    count.
    """
    scope = job_scope(run_path)
    if context == "matrix":
        return [
            value for path, value in values
            if path[-1] == name and "matrix" in path and path[:len(scope)] == scope
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
        # name the expression uses.
        return [
            value for path, value in values
            if "inputs" in path
            and (path[-1] == name
                 or (len(path) >= 2 and path[-1] == "default" and path[-2] == name))
        ]
    # `vars` are repository settings; nothing in the file to scan.
    return []


def is_a_plain_reference(expression):
    """Whether this expression is just value names, and not a command built here.

    `${{ matrix.extra_deps }}` and `${{ a || b }}` name values whose contents
    ARE the command. `${{ format('sudo apt-get install -y {0}', matrix.package) }}`
    assembles one instead: the `run:` text holds no apt invocation and the
    matrix value is a bare word, so checking either of them checks nothing.
    """
    remainder = CONTEXT_REFERENCE.sub("", without_literals(expression)[3:-2])
    return re.sub(r"\|\||&&|''|\s+", "", remainder) == ""


def command_segments(line):
    """The line's WORDS, split at its unquoted separators.

    Words rather than strings: rebuilding a segment by joining values throws
    away the quoting, and `"${{ github.ref }}"` then reads as a bare expression
    supplying the command rather than as the literal argument of a `[[ ]]`
    test. Built on the lexer so a `;` inside quotes stays part of its word.
    """
    try:
        words = lex_words(line)
    except ValueError:
        return []
    segments = [[]]
    for word in words:
        if word.value in SEPARATORS and not word.quoted:
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


def written_before_the_command(words):
    """Whether this segment holds a command of its own, before any expression.

    Assignments and redirections come BEFORE a command without being one:
    `FLAG=1 ${{ env.command }}` runs the value with a variable set for it, so
    the expression still supplies the command. Anything else written here means
    the command is this segment's and the expression is data in it.
    """
    index = 0
    while index < len(words):
        token = words[index].value
        if token in REDIRECTIONS and not words[index].quoted:
            index += 2
            continue
        if "=" in token and not token.startswith("="):
            index += 1
            continue
        if token.isdigit() and index + 1 < len(words) and words[index + 1].value in REDIRECTIONS:
            index += 1
            continue
        # The first word that is not a prefix decides it. A QUOTED expression
        # is a literal argument rather than a command being substituted:
        # `[[ "${{ github.ref }}" == refs/tags/* ]]` is a test, not a command
        # the expression supplies.
        return not (WHOLE_EXPRESSION.match(token) and not words[index].quoted)
    # Nothing but prefixes: assignments alone ARE the command — `OUT=x.apk`
    # sets a variable and runs nothing else — so the segment is written here.
    return True


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

        # Per SEGMENT, not per scalar and not per line: a line may hold
        # ordinary text AND a command after a separator that is nothing but an
        # expression, as in `echo preparing; ${{ env.command }}`.
        for offset, line_text in enumerate(value.value.splitlines()):
            # MASKED before segmenting: `${{ a || b }}` holds a `||` that is
            # part of the expression, not a shell separator, and splitting the
            # raw line tears the expression in half.
            masked_line, all_expressions = mask_expressions(line_text)
            for segment in command_segments(masked_line):
                expressions = [
                    all_expressions[int(position)]
                    for word in segment
                    for position in re.findall(EXPRESSION_PREFIX + r"(\d+)__", word.value)
                ]
                if not expressions:
                    continue
                # Whether anything of the command is written HERE.
                # `echo ${{ github.ref }}` is an ordinary command with a value
                # interpolated into it, and its apt is read as usual; a segment
                # that is nothing but an expression has no command here at all.
                if written_before_the_command(segment):
                    continue

                for expression in expressions:
                    for context, name in referenced_values(expression):
                        for referenced in resolve_reference(run_path, context, name, values):
                            if id(referenced) not in scanned:
                                scanned.add(id(referenced))
                                scan_scalar(path, referenced)
                    if not is_a_plain_reference(expression):
                        # The whole command is built by the expression, so it
                        # exists nowhere in the file to be checked. Announced,
                        # never silent.
                        emit("NOTICE", path, scalar_line(value, offset),
                             f"builds its command with an expression, assembled at run time "
                             f"and not checked: {expression}")


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
