#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generic utilities for working with text protobufs (without schema).
"""

import argparse
import collections
import dataclasses
import enum
import re
import sys
from collections.abc import Iterable, Iterator, Sequence
from pathlib import Path
from typing import Any


class TokenType(enum.Enum):
    FIELD_NAME = 0  # includes trailing ':'
    START_BLOCK = 1  # '<' or '{'
    END_BLOCK = 2  # '>' or '}'
    STRING_VALUE = 3  # quoted text, e.g. "string"
    OTHER_VALUE = 4  # non-string value
    SPACE = 5  # [ \t]*
    NEWLINE = 6  # [\n\r]


@dataclasses.dataclass
class Token(object):
    text: str
    type: TokenType


_FIELD_NAME_RE = re.compile(r"[a-zA-Z_][a-zA-Z0-9_]*:")
_SPACE_RE = re.compile(r"[ \t]+")
_NEWLINE_RE = re.compile(r"\r?\n")
_STRING_RE = re.compile(r'"([^\\"]|\\.)*"')  # Allow escaped quotes inside
_VALUE_RE = re.compile(r"[^ \t\r\n]+")  # Anything text that is not space


def _lex_line(line: str) -> Iterable[Token]:
    while line:  # is not empty
        next_char = line[0]

        if next_char in {"<", "{"}:
            yield Token(text=next_char, type=TokenType.START_BLOCK)
            line = line[1:]
            continue

        if next_char in {">", "}"}:
            yield Token(text=next_char, type=TokenType.END_BLOCK)
            line = line[1:]
            continue

        field_match = _FIELD_NAME_RE.match(line)
        if field_match:
            field_name = field_match.group(0)
            yield Token(text=field_name, type=TokenType.FIELD_NAME)
            line = line[len(field_name) :]
            continue

        string_match = _STRING_RE.match(line)
        if string_match:
            string = string_match.group(0)
            yield Token(text=string, type=TokenType.STRING_VALUE)
            line = line[len(string) :]
            continue

        value_match = _VALUE_RE.match(line)
        if value_match:
            value = value_match.group(0)
            yield Token(text=value, type=TokenType.OTHER_VALUE)
            line = line[len(value) :]
            continue

        space_match = _SPACE_RE.match(line)
        if space_match:
            space = space_match.group(0)
            yield Token(text=space, type=TokenType.SPACE)
            line = line[len(space) :]
            continue

        newline_match = _NEWLINE_RE.match(line)
        if newline_match:
            newline = newline_match.group(0)
            yield Token(text=newline, type=TokenType.NEWLINE)
            line = line[len(newline) :]
            continue

        raise ValueError(f'[textpb.lex] Unrecognized text: "{line}"')


def yield_verbose(items: Iterable[Any]) -> Iterable[Any]:
    for item in items:
        print(f"{item}")
        yield item


def _lex(lines: Iterable[str]) -> Iterable[Token]:
    """Divides proto text into tokens."""
    for line in lines:
        yield from _lex_line(line)


class ParseError(ValueError):
    def __init__(self, msg: str):
        super().__init__(msg)


def _auto_dict(values: Sequence[Any]) -> Any:
    """Convert sequences of key-value pairs to dictionaries."""
    if len(values) == 0:
        return values

    if all(
        isinstance(v, dict) and v.keys() == {"key", "value"} for v in values
    ):
        # assume keys are unique quoted strings
        # 'key' and 'value' should not be repeated fields
        return {v["key"][0].text.strip('"'): v["value"][0] for v in values}

    return values


def _parse_block(tokens: Iterator[Token], top: bool) -> dict[str, Any]:
    """Parse text proto tokens into a structure.

    Args:
      tokens: lexical tokens, without any spaces/newlines.

    Returns:
      dictionary representation of text proto.
    """
    # Without a schema, we cannot deduce whether a field is scalar or
    # repeated, so treat them as repeated (maybe singleton).
    result = collections.defaultdict(list)

    while True:
        try:
            field = next(tokens)
        except StopIteration:
            if top:
                break
            else:
                raise ParseError(
                    "Unexpected EOF, missing '>' or '}' end-of-block"
                )

        if field.type == TokenType.END_BLOCK:
            if top:
                raise ParseError(
                    "Unexpected end-of-block at top-level before EOF."
                )
            break

        if field.type != TokenType.FIELD_NAME:
            raise ParseError(f"Expected a field name, but got {field}.")

        key = field.text[:-1]  # removes trailing ':'
        try:
            value_or_block = next(tokens)
        except StopIteration:
            raise ParseError(
                "Unexpected EOF, expecting a value or start-of-block."
            )

        value: dict[str, Any] | Token
        if value_or_block.type == TokenType.START_BLOCK:
            value = _parse_block(tokens, top=False)
        elif value_or_block.type in {
            TokenType.STRING_VALUE,
            TokenType.OTHER_VALUE,
        }:
            value = value_or_block  # a Token
        else:
            raise ParseError(f"Unexpected token: {value_or_block}")

        result[key].append(value)

    # End of block, post-process key-value pairs into dictionaries.
    return {k: _auto_dict(v) for k, v in result.items()}


def _parse_tokens(tokens: Iterator[Token]) -> dict[str, Any]:
    return _parse_block(tokens, top=True)


def parse(lines: Iterable[str]) -> dict[str, Any]:
    """Parse a text protobuf into a recursive dictionary.

    Args:
      lines: lines of text proto (spaces and line breaks are ignored)

    Returns:
      Structured representation of the proto.
      Fields are treated as either repeated (even if the original
      schema was scalar) or as key-value dictionaries.
    """
    # ignore spaces
    return _parse_tokens(
        token
        for token in _lex(lines)
        if token.type not in {TokenType.SPACE, TokenType.NEWLINE}
    )


def _main_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Parse any text proto and show its representation.",
        argument_default=None,
    )
    parser.add_argument(
        "input",
        type=Path,
        metavar="FILE",
        help="The text proto file to parse",
    )
    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def main(argv: Sequence[str]) -> int:
    args = _MAIN_ARG_PARSER.parse_args(argv)
    with open(args.input) as f:
        data = parse(f)
    print(data)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
