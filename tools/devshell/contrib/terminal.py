#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
A utility class for python scripts writing to terminal output.
"""

import os
import sys
from typing import Any


class Terminal:
    suppress = False

    @classmethod
    def bold(cls, text: str) -> str:
        return cls._style(text, 1)

    @classmethod
    def red(cls, text: str) -> str:
        return cls._style(text, 91)

    @classmethod
    def green(cls, text: str) -> str:
        return cls._style(text, 92)

    @classmethod
    def yellow(cls, text: str) -> str:
        return cls._style(text, 93)

    @classmethod
    def supports_color(cls) -> bool:
        return (
            sys.stdout.isatty()
            and sys.stderr.isatty()
            and not os.environ.get("NO_COLOR")
        )

    @classmethod
    def _print(cls, *args: Any, **kwargs: Any) -> None:
        if not cls.suppress:
            print(*args, **kwargs)

    @classmethod
    def fatal(cls, text: str) -> int:
        cls._print(cls.red(cls.bold("FATAL: ")) + text, file=sys.stderr)
        sys.exit(1)

    @classmethod
    def error(cls, text: str) -> None:
        cls._print(cls.red(cls.bold("ERROR: ")) + text, file=sys.stderr)

    @classmethod
    def warn(cls, text: str) -> None:
        cls._print(cls.yellow(cls.bold("WARNING: ")) + text, file=sys.stderr)

    @classmethod
    def info(cls, text: str) -> None:
        cls._print(cls.green(cls.bold("INFO: ")) + text)

    @classmethod
    def _style(cls, text: str, escape_code: int) -> str:
        if cls.supports_color():
            return f"\033[{escape_code}m{text}\033[0m"
        else:
            # If neither stdout nor stderr is not a tty then any styles likely
            # won't get rendered correctly when the text is eventually printed,
            # so don't apply the style.
            return text
