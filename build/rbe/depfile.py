#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generic utilities for working with depfiles
"""

import dataclasses
import enum
import re
from collections.abc import Callable, Iterable, Sequence
from pathlib import Path
from typing import AbstractSet


class LexError(ValueError):
    def __init__(self, msg: str):
        super().__init__(msg)


class ParseError(ValueError):
    def __init__(self, msg: str):
        super().__init__(msg)


class TokenType(enum.Enum):
    PATH = 0  # anything that isn't the others
    COLON = 1  # ':'
    SPACE = 2  # [ \t]
    NEWLINE = 3  # [\n\r]
    LINECONTINUE = 4  # '\\' (also start of escape-sequence)
    COMMENT = 5  # '#...'
    ESCAPED = 6  # e.g. '\t'


@dataclasses.dataclass
class Token(object):
    text: str
    type: TokenType


_SPACE_RE = re.compile(r"[ \t]+")
_NEWLINE_RE = re.compile(r"\r?\n")
_PATH_RE = re.compile(r"[a-zA-Z0-9_/.+-]+")
_COMMENT_RE = re.compile(r"#[^\n]*")


def _lex_line(line: str) -> Iterable[Token]:
    prev: Token | None = None
    while line:  # is not empty
        next_char = line[0]

        if prev and prev.type == TokenType.LINECONTINUE:
            newline_match = _NEWLINE_RE.match(line)
            if newline_match:
                newline = newline_match.group(0)
                yield prev
                prev = None
                yield Token(text=newline, type=TokenType.NEWLINE)
                line = line[len(newline) :]
                continue

            # otherwise, we don't handle escaped sequences yet
            new = Token(text=prev.text + next_char, type=TokenType.ESCAPED)
            raise LexError(f"Escape sequences are not handled yet: {new}")

        if next_char == ":":
            yield Token(text=next_char, type=TokenType.COLON)
            line = line[1:]
            continue

        if next_char == "\\":
            prev = Token(text=next_char, type=TokenType.LINECONTINUE)
            # do not yield yet, look at text that follows
            line = line[1:]
            continue

        space_match = _SPACE_RE.match(line)
        if space_match:
            spaces = space_match.group(0)
            yield Token(text=spaces, type=TokenType.SPACE)
            line = line[len(spaces) :]
            continue

        newline_match = _NEWLINE_RE.match(line)
        if newline_match:
            newline = newline_match.group(0)
            yield Token(text=newline, type=TokenType.NEWLINE)
            line = line[len(newline) :]
            continue

        comment_match = _COMMENT_RE.match(line)
        if comment_match:
            comment = comment_match.group(0)
            yield Token(text=comment, type=TokenType.COMMENT)
            line = line[len(comment) :]
            continue

        path_match = _PATH_RE.match(line)
        if path_match:
            path = path_match.group(0)
            yield Token(text=path, type=TokenType.PATH)
            line = line[len(path) :]
            continue

        raise LexError(f'[depfile.lex] Unrecognized text: "{line}"')

    if prev:
        yield prev


def lex(lines: Iterable[str]) -> Iterable[Token]:
    """Divides depfile text into tokens."""
    for line in lines:
        yield from _lex_line(line)


def _transform_paths(
    tokens: Iterable[Token], transform: Callable[[str], str]
) -> Iterable[Token]:
    """Space-preserving path transformation."""
    for token in tokens:
        if token.type == TokenType.PATH:
            yield Token(text=transform(token.text), type=TokenType.PATH)
        else:
            yield token


def unlex(tokens: Iterable[Token]) -> str:
    """Concatenates tokens' text."""
    return "".join(token.text for token in tokens)


def transform_paths(text: str, transform: Callable[[str], str]) -> str:
    """Applies an arbitrary transformation to depfile paths."""
    return unlex(
        _transform_paths(lex(text.splitlines(keepends=True)), transform)
    )


def consume_line_continuations(toks: Iterable[Token]) -> Iterable[Token]:
    """Filter out line-continuations, as if there were long lines."""
    line_cont = None
    for tok in toks:
        if line_cont is not None:
            if tok.type == TokenType.NEWLINE:
                line_cont = None
                continue

        if tok.type == TokenType.LINECONTINUE:
            line_cont = tok
            continue

        yield tok


@dataclasses.dataclass
class Dep(object):
    targets: Sequence[Token]
    colon: Token
    deps: Sequence[Token] = dataclasses.field(default_factory=list)

    @property
    def target_paths(self) -> Sequence[Path]:
        return [Path(d.text) for d in self.targets]

    @property
    def deps_paths(self) -> Sequence[Path]:
        return [Path(d.text) for d in self.deps]

    @property
    def is_phony(self) -> bool:
        return len(self.deps) == 0

    @property
    def absolute_paths(self) -> AbstractSet[Path]:
        return {
            p for p in [*self.target_paths, *self.deps_paths] if p.is_absolute()
        }


class _ParserState(enum.Enum):
    EXPECTING_FIRST_TARGET = 0
    EXPECTING_TARGET_OR_COLON = 1
    EXPECTING_DEP_OR_NEWLINE = 2
    DONE = 3


def _parse_one_dep(toks: Iterable[Token]) -> Dep | None:
    """Parse a single dependency.

    Args:
      toks: tokens, already filtered through consume_line_continuations().

    Returns:
      A Dep, or just None if token stream ended cleanly.

    Raises:
      ParseError if there are any syntax errors.
    """
    # parser states
    state: _ParserState = _ParserState.EXPECTING_FIRST_TARGET

    targets = []
    colon: Token
    deps = []

    for tok in toks:
        if state == _ParserState.EXPECTING_FIRST_TARGET:
            # ignore blank lines
            if tok.type == TokenType.NEWLINE:
                continue
            if tok.type == TokenType.PATH:
                targets.append(tok)
                state = _ParserState.EXPECTING_TARGET_OR_COLON
                continue

        elif state == _ParserState.EXPECTING_TARGET_OR_COLON:
            if tok.type == TokenType.PATH:
                targets.append(tok)
                continue
            if tok.type == TokenType.COLON:
                colon = tok
                state = _ParserState.EXPECTING_DEP_OR_NEWLINE
                continue

        elif state == _ParserState.EXPECTING_DEP_OR_NEWLINE:
            if tok.type == TokenType.PATH:
                deps.append(tok)
                continue
            if tok.type == TokenType.NEWLINE:
                state = _ParserState.DONE
                break  # newline terminator, done

        else:
            assert False, f"Internal error: unknown parser state: {state}"

        # reaching this point is a parse error
        raise ParseError(f"In state {state}: Unexpected token: {tok}")

    if state == _ParserState.EXPECTING_FIRST_TARGET:
        return None
    elif state != _ParserState.DONE:
        raise ParseError(
            f"Expecting a terminal line-break, but reached end of token stream."
        )

    return Dep(targets=targets, colon=colon, deps=deps)


def _parse_filtered_tokens(toks: Iterable[Token]) -> Iterable[Dep]:
    while True:
        dep = _parse_one_dep(toks)
        if dep is None:
            return
        yield dep


def _parse_tokens(toks: Iterable[Token]) -> Iterable[Dep]:
    yield from _parse_filtered_tokens(
        tok
        for tok in consume_line_continuations(toks)
        # keep NEWLINEs as separators between deps
        if tok.type not in {TokenType.SPACE, TokenType.COMMENT}
    )


def parse_lines(lines: Iterable[str]) -> Iterable[Dep]:
    """Parse lines of a depfile.

    Args:
      lines: lines of depfile (including trailing newline)

    Yields:
      Dep objects.

    Raises:
      ParseError or LexError if input is malformed.
    """
    yield from _parse_tokens(lex(lines))


def absolute_paths(deps: Iterable[Dep]) -> AbstractSet[Path]:
    unique_paths: set[Path] = set()
    for d in deps:
        unique_paths.update(d.absolute_paths)
    return unique_paths
