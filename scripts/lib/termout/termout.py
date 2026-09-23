# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Terminal output library for overwriting content in a window.

This module contains functionality to show a fixed number of lines
in the user's terminal. The output continually overwrites itself. The
library can be used to write very basic output-only terminal UIs.

Typical Usage:
    if termout.is_valid():
        termout.init()
        termout.write_lines([
          'Hello, world'
        ])
        time.sleep(1)
        termout.write_lines([
          'This is,',
          '  a test...',
        ])
        time.sleep(1)
        termout.write_lines([
            'Goodbye'
        ])
        time.sleep(1)
"""

import atexit
import io
import os
import shutil
import sys
import termios
import threading
from dataclasses import dataclass

import colorama


class TerminalError(Exception):
    """Raised when there is an exception related to terminal output."""


def is_valid() -> bool:
    """Determine if it is valid to use termout in this environment.

    Returns:
        True if stdout goes somewhere we support (like a TTY), and
        False otherwise.
    """
    return os.isatty(sys.stdout.fileno()) and os.environ.get("TERM") != "dumb"


def _suspend_echo() -> None:
    """Stop echoing to the terminal and hide the cursor.

    Automatically installs a routine to run at process exit to
    restore echoing and cursor visibility.
    """
    try:
        fd = sys.stdin.fileno()
        if not os.isatty(fd):
            return
        orig_flags = termios.tcgetattr(fd)
        new_flags = termios.tcgetattr(fd)
        new_flags[3] = new_flags[3] & ~termios.ECHO
        termios.tcsetattr(fd, termios.TCSANOW, new_flags)

        def cleanup() -> None:
            print("\r")
            try:
                termios.tcsetattr(fd, termios.TCSANOW, orig_flags)
            except (termios.error, OSError):
                pass

        atexit.register(cleanup)
    except (termios.error, OSError):
        pass


_init: bool = False


def init() -> None:
    """Initialize terminal writing, installing handlers to restore settings at exit.

    Raises:
        TerminalError: stdout is not a valid output for this library.
    """
    global _init
    if _init:
        raise TerminalError("init() may only be called once")
    if not is_valid():
        raise TerminalError(
            "The output stream does not seem to be a valid output. termout must be used on a TTY."
        )
    colorama.init()
    _suspend_echo()
    _init = True


def is_init() -> bool:
    """Return if termout is initialized.

    Returns:
        bool: True if output is initialized, false otherwise.
    """
    return _init


@dataclass
class Size:
    """Represents the width and height of a terminal window."""

    columns: int
    lines: int


def get_size() -> Size:
    """Get the size of the terminal output.

    Returns:
        A Size object containing columns and lines
    """
    size = shutil.get_terminal_size()
    return Size(size.columns, size.lines)


_last_line_count: int | None = None
_write_lock: threading.Lock = threading.Lock()


def reset() -> None:
    """Resets the global state of the library.

    termout is stateful, and for testing purposes it is useful to
    reset the internal state.
    """
    global _last_line_count
    with _write_lock:
        _last_line_count = None


_CLEAR_SCREEN_TO_END_MODE = 0


def write_lines(
    lines: list[str],
    prepend: list[str] | None = None,
    size: Size | None = None,
) -> None:
    """Write a list of lines to the terminal.

    Lines will be truncated if they fail to fit within the current
    terminal window, excluding control characters.

    This function is thread-safe.

    Args:
        lines: The list of lines to write.
        prepend: Optional list of lines to prepend to output.
        size: Optional terminal size override.
    """
    global _last_line_count
    with _write_lock:
        if size is None:
            size = get_size()

        write_buffer = io.StringIO()

        if _last_line_count:
            write_buffer.writelines(
                [
                    # Go to beginning of line
                    "\r",
                    # Move up, but only if we are on a new line.
                    # If the cursor was left on the same line as
                    # the only text (_last_line_count == 1), then
                    # moving to the beginning of the line was sufficient
                    # and attempting to scroll with an offset of 0
                    # will delete the previous line erroneously!
                    (
                        colorama.Cursor.UP(_last_line_count - 1)
                        if _last_line_count > 1
                        else ""
                    ),
                    # Clear to the end of the screen so we do not leave old
                    # text on the screen.
                    colorama.ansi.clear_screen(_CLEAR_SCREEN_TO_END_MODE),
                ]
            )

        for line in prepend or []:
            print(line + colorama.Style.RESET_ALL, file=write_buffer)

        formatted_lines: list[str] = []

        for line in lines:
            printing = True
            count = 0
            max_index: int = 0
            for index, ch in enumerate(line):
                if ch == "\x1b":
                    printing = False
                elif not printing and ch in ["m", "J", "K", colorama.ansi.BEL]:
                    # Detect the end of an ANSI escape sequence.
                    printing = True
                elif printing:
                    count += 1
                if count > size.columns:
                    break
                max_index = index + 1

            formatted_lines.append(
                "\r" + line[:max_index] + colorama.Style.RESET_ALL
            )

        write_buffer.writelines(["\n".join(formatted_lines)])
        write_buffer.flush()
        sys.stdout.writelines([write_buffer.getvalue()])
        sys.stdout.flush()
        _last_line_count = len(lines)
