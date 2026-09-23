# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Structured reporting, TTY/ASCII detection, and remediation hints for Git hooks."""

from __future__ import annotations

import sys
from collections.abc import Sequence
from typing import Any, TextIO


def _is_utf8_supported(stream: Any) -> bool:
    """Determines whether the given stream supports UTF-8 characters."""
    encoding = getattr(stream, "encoding", None) or "utf-8"
    try:
        "🧹⚠️❌".encode(encoding)
        return True
    except (UnicodeEncodeError, LookupError):
        return False


def _format_labeled_message(message: str, marker: str) -> str:
    """Formats a message with a canonical marker."""
    lines = message.strip().splitlines()
    if not lines:
        return ""
    first_line = f"{marker} {lines[0]}" if marker else lines[0]
    return "\n".join([first_line, *lines[1:]])


class ConsoleReporter:
    """Human-readable console reporter supporting TTY and UTF-8/ASCII degradation."""

    def __init__(
        self,
        *,
        stdout_stream: TextIO | None = None,
        stderr_stream: TextIO | None = None,
        unicode: bool | None = None,
    ) -> None:
        self.errors: list[str] = []
        self.warnings: list[str] = []
        self.stdout_stream = (
            sys.stdout if stdout_stream is None else stdout_stream
        )
        self.stderr_stream = (
            sys.stderr if stderr_stream is None else stderr_stream
        )
        self._unicode = unicode

    def _stream_unicode(self, stream: TextIO) -> bool:
        """Determines whether the given stream should use unicode output."""
        if self._unicode is not None:
            return self._unicode
        return bool(
            getattr(stream, "isatty", lambda: False)()
            and _is_utf8_supported(stream)
        )

    @property
    def has_errors(self) -> bool:
        """Returns True if any errors or fatal conflicts were reported."""
        return bool(self.errors)

    def _print_stream(self, text: str, *, file: TextIO) -> None:
        """Safely prints text to the given stream, falling back on encoding errors."""
        try:
            print(text, file=file)
        except UnicodeEncodeError:
            encoding = getattr(file, "encoding", None) or "ascii"
            print(
                text.encode(encoding, errors="replace").decode(encoding),
                file=file,
            )

    def on_files_restaged(self, files: Sequence[str], message: str) -> None:
        """Prints notification when files have been modified and re-staged."""
        if not files and not message:
            return
        marker = "🧹" if self._stream_unicode(self.stdout_stream) else "[FIXED]"
        if files:
            file_list = ", ".join(files)
            text = f"{message}: {file_list}" if message else file_list
        else:
            text = message
        formatted = _format_labeled_message(text, marker)
        if formatted:
            self._print_stream(formatted, file=self.stdout_stream)

    def on_partial_staging_conflict(
        self,
        files: Sequence[str],
        message: str,
        remediation_cmds: Sequence[str] = (),
    ) -> None:
        """Prints error and remediation when partially staged files fail checks."""
        msg = message or "Partially staged files failed checks"
        self.errors.append(msg)
        marker = "❌" if self._stream_unicode(self.stderr_stream) else "[ERROR]"
        lines = [_format_labeled_message(msg, marker)]
        if files:
            lines.append("  Offending files:")
            for f in files:
                lines.append(f"    - {f}")
        if remediation_cmds:
            lines.append("  Or fix directly:")
            for cmd in remediation_cmds:
                lines.append(f"    {cmd}")
        self._print_stream("\n".join(lines), file=self.stderr_stream)

    def on_warning(self, message: str) -> None:
        """Prints advisory warning message to stderr."""
        clean = message.strip()
        if not clean:
            return
        self.warnings.append(clean)
        marker = "⚠️" if self._stream_unicode(self.stderr_stream) else "[WARN]"
        formatted = _format_labeled_message(clean, marker)
        if formatted:
            self._print_stream(formatted, file=self.stderr_stream)

    def on_error(self, message: str) -> None:
        """Prints error message to stderr."""
        clean = message.strip()
        if not clean:
            return
        self.errors.append(clean)
        marker = "❌" if self._stream_unicode(self.stderr_stream) else "[ERROR]"
        formatted = _format_labeled_message(clean, marker)
        if formatted:
            self._print_stream(formatted, file=self.stderr_stream)

    def finish(self) -> None:
        """Finalizes console reporter and flushes output streams."""
        if hasattr(self.stdout_stream, "flush"):
            self.stdout_stream.flush()
        if hasattr(self.stderr_stream, "flush"):
            self.stderr_stream.flush()


__all__ = [
    "ConsoleReporter",
]
