# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Checks a shac Starlark file for fuchsia-specific issues.

Since Starlark's syntax is a subset of Python's, we can use the Python ast
library to parse Starlark.
"""

import ast
import collections
import json
import sys

COMMON_DOT_STAR = "scripts/shac/common.star"

# Add a comment of this form after the closing parenthesis of a `print()` call
# to allow it to be committed.
ALLOW_PRINT_DIRECTIVE = "# allow-print"


def main():
    findings = collections.defaultdict(list)
    path = sys.argv[1]
    with open(path) as f:
        contents = f.read()
        lines = contents.splitlines()

    tree = ast.parse(contents)
    for node in ast.walk(tree):
        if (
            isinstance(node, ast.Call)
            and ast.unparse(node.func) == "ctx.emit.finding"
            and not any(kw.arg == "message" for kw in node.keywords)
        ):
            # shac requires `message` except for findings emitted by formatters,
            # in which case it provides a default message saying to run `shac
            # fmt`. However, `shac fmt` isn't exposed to fuchsia developers
            # directly, instead they should use `fx format-code`, so we should
            # never fall back to the default message.
            findings[node].append(
                "`message` argument to `ctx.emit.finding()` must be set."
            )

        if (
            isinstance(node, ast.Call)
            and ast.unparse(node.func) == "print"
            and not lines[node.end_lineno - 1].endswith(ALLOW_PRINT_DIRECTIVE)
        ):
            findings[node].append(
                "Do not commit shac check code that calls `print()`, "
                "it's only to be used for debugging."
            )

        if (
            isinstance(node, ast.Call)
            and ast.unparse(node.func) == "ctx.os.exec"
            and path != COMMON_DOT_STAR
        ):
            # All paths passed to `os_exec()` must be absolute in order for
            # inherited checks in //vendor repositories to work. It's nontrivial
            # to validate that statically, so instead we validate it at runtime
            # in a custom wrapper function, and statically enforce that that
            # wrapper function is used instead of `os_exec()`.
            findings[node].append(
                f"Don't call `ctx.os.exec()` directly, call `os_exec()` "
                f"from //{COMMON_DOT_STAR}."
            )

    print(
        json.dumps(
            [
                {
                    "message": msg,
                    "line": node.lineno,
                    "col": node.col_offset + 1,
                    "end_line": node.end_lineno,
                    "end_col": node.end_col_offset + 1,
                }
                for node, msgs in findings.items()
                for msg in msgs
            ],
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
