# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Starlark related utilities."""

import typing as T


def to_starlark_expr(v: T.Any, indent: str = "") -> str:
    """Convert a Python value to the corresponding Starlark expression.

    Args:
        v: input value.
        indent: optional starting indentation string. Empty by default.
            this is only used when the output expression spans several
            lines.
    Returns:
        the corresponding Starlark expression as a string.
    Raises:
        ValueError if the input value cannot be represented by a
        Starlark expression. In particular set() values are not
        supported by the Starlark language.
    """
    return StarlarkFormatter.format(v, indent)


class StarlarkFormatter(object):
    """Implementation class for to_starlark_expr().

    Used to avoid redundant string allocations and concatenations during
    formatting. Instead, each instance contains a single output buffer that
    is appended to by the various _format_xxx() methods.
    """

    @staticmethod
    def format(v: T.Any, indent: str) -> str:
        f = StarlarkFormatter(indent)
        f._format_value(v)
        return f._result

    def __init__(self, indent: str = "") -> None:
        self._result = ""
        self._indent = indent

    def _format_value(self, v: T.Any) -> None:
        if isinstance(v, list):
            self._format_list(v)
        elif isinstance(v, dict):
            self._format_dict(v)
        elif isinstance(v, str):
            self._format_string(v)
        elif isinstance(v, bool) or isinstance(v, int):
            self._result += str(v)
        else:
            raise ValueError(
                f"Unknown type of input value: {v} (type={type(v)})"
            )

    def _format_string(self, v: str) -> None:
        self._result += '"'
        for c in v:
            if ord(c) < 32:  # non-printable characters.
                if c == "\b":
                    c = "\\b"
                elif c == "\r":
                    c = "\\r"
                elif c == "\f":
                    c = "\\f"
                elif c == "\n":
                    c = "\\n"
                elif c == "\t":
                    c = "\\t"
                else:
                    c = "\\u%02x" % ord(c)
            elif c == '"':
                c = '\\"'
            elif c == "\\":
                c = "\\\\"
            self._result += c
        self._result += '"'

    def _format_list(self, v: list[T.Any]) -> None:
        if not v:
            self._result += "[]"
            return

        if len(v) == 1:
            self._result += "["
            self._format_value(v[0])
            self._result += "]"
            return

        comma = "["
        self._indent += "    "
        for item in v:
            self._result += f"{comma}\n{self._indent}"
            self._format_value(item)
            comma = ","

        self._indent = self._indent[:-4]
        self._result += f"\n{self._indent}]"

    def _format_dict(self, v: dict[T.Any, T.Any]) -> None:
        if not v:
            self._result += "{}"
            return

        if len(v) == 1:
            key, value = v.popitem()
            self._result += "{"
            self._format_value(key)
            self._result += ": "
            self._format_value(value)
            self._result += "}"
            return

        comma = "{"
        self._indent += "    "
        for key, value in v.items():
            self._result += f"{comma}\n{self._indent}"
            self._format_value(key)
            self._result += ": "
            self._format_value(value)
            comma = ","

        self._indent = self._indent[:-4]
        self._result += f"\n{self._indent}}}"
