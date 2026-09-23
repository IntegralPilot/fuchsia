# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import io
import re

_UP_REGEX = re.compile(r"\[(\d+)A")


class Terminal:
    """Terminal simulator for testing.

    This class can be set as stdout to simulate a terminal. It implements only
    the bare minimum ANSI escape codes needed to test termout, as follows:
    - Style commands; ignored.
    - Cursor up, with arbitrary offset.
    - Clear remainder of screen, available only when at the beginning of a line.
    - Next line, valid only as \\n.

    Attributes:
        lines (List[str]): The list of lines that would show on a terminal.
    """

    def __init__(self, width: int) -> None:
        self._width = width
        self.lines: list[str] = [""]
        self._line_position: int = 0
        self._current_write = io.StringIO()

    def write(self, value: str) -> int:
        """Implementation of io write.

        Args:
            value (str): Value to write.

        Returns:
            int: Number of bytes written.
        """
        return self._current_write.write(value)

    def writelines(self, value: list[str]) -> None:
        """Implementation of io writelines.

        Args:
            value (List[str]): Value to write.
        """
        self.write("".join(value))

    def flush(self) -> None:
        """Implementation of io flush.

        To ensure that the library flushes appropriately, the lines field
        is updated only on flush.
        """
        chars = [ch for ch in self._current_write.getvalue()]
        chars.reverse()

        # Determine if we know that we are at the beginning of a line.
        at_front = True

        while chars:
            ch = chars.pop()
            if ch == "\n":
                self._line_position += 1
                while len(self.lines) <= self._line_position:
                    self.lines.append("")
            elif ch == "\r":
                at_front = True
            elif ch == "\x1B":
                # Handle escape sequences.
                next_ch = chars.pop()
                code = ""
                while next_ch and not next_ch.isalpha():
                    code += next_ch
                    next_ch = chars.pop()
                if next_ch:
                    code += next_ch
                if not code:
                    raise TerminalError("Empty escape code")
                if code[-1] == "m":
                    # Color style, skip
                    pass
                elif (m := _UP_REGEX.match(code)) is not None:
                    # Go up N lines
                    value = int(m.group(1))
                    if value == 0:
                        raise TerminalError(
                            "It is invalid to go up 0 lines in the terminal"
                        )
                    self._line_position -= value
                elif code == "[0J":
                    if not at_front:
                        raise TerminalError(
                            "Can only erase to end of screen if we are at the front of a line."
                        )
                    # Clear to end of screen
                    for l in range(self._line_position, len(self.lines)):
                        self.lines[l] = ""
                else:
                    raise TerminalError(f"Unknown escape code \\x1B{code}")
            else:
                at_front = False
                if len(self.lines[self._line_position]) >= self._width:
                    # Simulate a newline to go to the next line.
                    chars.append(ch)
                    chars.append("\n")
                else:
                    self.lines[self._line_position] += ch

        self._current_write = io.StringIO()


class TerminalError(Exception):
    """There was an error simulating a terminal."""
