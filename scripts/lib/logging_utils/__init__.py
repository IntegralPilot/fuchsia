# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generic logging utilities with ANSI color support."""

from __future__ import annotations

import logging
import os
import re
import sys
from typing import Callable

from colorama import Fore, Style

_LEVEL_COLORS: dict[int, str] = {
    logging.DEBUG: Fore.LIGHTBLACK_EX,
    logging.INFO: Fore.GREEN,
    logging.WARNING: Fore.YELLOW,
    logging.ERROR: Fore.RED + Style.BRIGHT,
    logging.CRITICAL: Fore.RED + Style.BRIGHT,
}

_PREFIX_RE: re.Pattern[str] = re.compile(r"^(\[[^\]\n]+\])")
_TIMESTAMP_RE: re.Pattern[str] = re.compile(
    r"(\b(?:\d{4}-)?\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:[.,]\d{3})?\b)"
)
_TARGET_NAME_RE: re.Pattern[str] = re.compile(
    r"\b(fuchsia-(?:emulator|[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}|[a-zA-Z0-9]+(?:-[a-zA-Z0-9]+)*))\b"
)
_PREFIX_REPL: str = f"{Fore.CYAN}\\1{Style.RESET_ALL}"
_TIMESTAMP_REPL: str = f"{Fore.LIGHTBLACK_EX}\\1{Style.RESET_ALL}"
_TARGET_REPL: str = f"{Fore.CYAN}\\1{Style.RESET_ALL}"

_RESULT_COLORS: dict[str, str] = {
    "PASS": f"{Fore.GREEN}{Style.BRIGHT}PASS{Style.RESET_ALL}",
    "FAIL": f"{Fore.RED}{Style.BRIGHT}FAIL{Style.RESET_ALL}",
    "ERROR": f"{Fore.RED}{Style.BRIGHT}ERROR{Style.RESET_ALL}",
    "SKIP": f"{Fore.YELLOW}SKIP{Style.RESET_ALL}",
}
_TEST_RESULT_RE: re.Pattern[str] = re.compile(r"\b(PASS|FAIL|ERROR|SKIP)\b")
_RET_CODE_RE: re.Pattern[str] = re.compile(r"\[ret: (\d+)\]")


def _colorize_ret(match: re.Match[str]) -> str:
    code = match.group(1)
    color = Fore.GREEN if code == "0" else Fore.RED
    return f"[{color}ret: {code}{Style.RESET_ALL}]"


def lacewing_log_formatter(formatted: str) -> str:
    """Applies Lacewing-specific regex color highlights to a formatted log line."""
    # Colorize leading bracketed prefix e.g. [GeneratedLocalTestbed] or [Mobly Driver]
    formatted = _PREFIX_RE.sub(_PREFIX_REPL, formatted)

    # Dim / light-black timestamp e.g. 08-12 13:38:09.334
    formatted = _TIMESTAMP_RE.sub(_TIMESTAMP_REPL, formatted)

    # Colorize Fuchsia target nodenames (e.g. fuchsia-1a7d-1303-a909, fuchsia-emulator)
    formatted = _TARGET_NAME_RE.sub(_TARGET_REPL, formatted)

    # Highlight test completion results and return codes
    if "[Test]" in formatted or "Completed:" in formatted:
        formatted = _TEST_RESULT_RE.sub(
            lambda m: _RESULT_COLORS[m.group(1)], formatted
        )
        formatted = _RET_CODE_RE.sub(_colorize_ret, formatted)

    return formatted


def supports_color(stream: object = None) -> bool:
    """Returns True if the given stream (or stdout) supports ANSI color output."""
    if os.environ.get("NO_COLOR"):
        return False
    if os.environ.get("TERM") == "dumb":
        return False
    if (
        os.environ.get("FORCE_COLOR")
        or os.environ.get("CLICOLOR_FORCE", "0") != "0"
    ):
        return True
    if os.environ.get("FUCHSIA_FX_TEST_RUN") == "1":
        return True
    if stream is None:
        stream = sys.stdout
    return bool(hasattr(stream, "isatty") and stream.isatty())


class ColoredFormatter(logging.Formatter):
    """A logging formatter that applies ANSI colors when color is supported."""

    def __init__(
        self,
        fmt: str | None = None,
        datefmt: str | None = None,
        prefix: str | None = None,
        use_color: bool | None = None,
        format_func: Callable[[str], str] | None = lacewing_log_formatter,
    ) -> None:
        super().__init__(fmt=fmt, datefmt=datefmt)
        self.prefix: str | None = prefix
        self.use_color: bool = (
            use_color if use_color is not None else supports_color(sys.stdout)
        )
        self.format_func: Callable[[str], str] | None = format_func

    def format(self, record: logging.LogRecord) -> str:
        if not self.use_color:
            formatted = super().format(record)
        else:
            orig_levelname = record.levelname
            try:
                color = _LEVEL_COLORS.get(record.levelno, "")
                record.levelname = f"{color}{record.levelname}{Style.RESET_ALL}"
                formatted = super().format(record)
                if self.format_func is not None:
                    formatted = self.format_func(formatted)
            finally:
                record.levelname = orig_levelname

        if self.prefix:
            formatted = f"{self.prefix}{formatted}"
        return formatted


def colorize_logger(
    logger: logging.Logger,
    prefix: str | None = None,
    format_func: Callable[[str], str] | None = lacewing_log_formatter,
) -> None:
    """Updates any console StreamHandler on the logger to use ColoredFormatter.

    Leaves FileHandlers untouched to ensure on-disk logs remain plain text.

    Args:
        logger: The Logger instance whose StreamHandlers to colorize.
        prefix: Optional prefix for the formatter.
        format_func: Optional function to format/transform the log message after standard
            formatting. Defaults to `lacewing_log_formatter`.
    """
    for handler in logger.handlers:
        if isinstance(handler, logging.StreamHandler) and not isinstance(
            handler, logging.FileHandler
        ):
            fmt = getattr(handler.formatter, "_fmt", None)
            datefmt = getattr(handler.formatter, "datefmt", None)
            handler.setFormatter(
                ColoredFormatter(
                    fmt=fmt,
                    datefmt=datefmt,
                    prefix=prefix,
                    format_func=format_func,
                )
            )


__all__ = [
    "ColoredFormatter",
    "colorize_logger",
    "lacewing_log_formatter",
    "supports_color",
]
