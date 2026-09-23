# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import dataclasses
import os
import shlex
import typing as T

# Default separators to split shell commands by.
_DEFAULT_SPLIT_SEPARATORS = {"&&", "||"}


def _split_by_separators(
    cmd_str: str, separators: set[str] = _DEFAULT_SPLIT_SEPARATORS
) -> list[str]:
    """Splits the command string by unquoted separators.

    Returns a list of strings (the chunks) and separators.

    Example:
        _split_by_separators("echo a && echo b") == ['echo a', '&&', 'echo b']
    """

    # Use shlex in non-posix mode to preserve quotes, ensuring we can distinguish
    # '&&' (quoted string) from && (operator).
    #
    # NOTE: In this case we can't have punctuation_chars=True, because it will
    # split on parentheses, which are used in target labels. For example the
    # following command would be split incorrectly:
    #
    #   --remote-flag=--label='//tools/create:create_bin.actual(//build/toolchain:host_x64)'
    #
    lexer = shlex.shlex(cmd_str, posix=False, punctuation_chars=False)
    lexer.whitespace_split = True

    chunks = []
    current_chunk: list[str] = []

    for token in lexer:
        if token in separators:
            if current_chunk:
                chunks.append(" ".join(current_chunk))
                current_chunk = []
            chunks.append(token)
        else:
            current_chunk.append(token)

    if current_chunk:
        chunks.append(" ".join(current_chunk))

    return chunks


class ShellCommand:
    """Represents a shell command."""

    def __init__(self, command: str | list[str]):
        if isinstance(command, list):
            self._str = shlex.join(command)
        else:
            self._str = command

    def __str__(self) -> str:
        return self._str

    def __repr__(self) -> str:
        return f"ShellCommand({self._str!r})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, ShellCommand):
            return NotImplemented
        return self._str == other._str

    def split(
        self, separators: set[str] = _DEFAULT_SPLIT_SEPARATORS
    ) -> list["ShellCommand"]:
        """Splits the command by top-level && and || separators.

        Returns separated subcommands as ShellCommand objects. Separators are dropped.
        """
        chunks = _split_by_separators(self._str, separators)
        result: list["ShellCommand"] = []
        for chunk in chunks:
            if chunk in separators:
                continue
            else:
                result.append(ShellCommand(chunk))
        return result

    def unwrap(self) -> T.Optional["ShellCommand"]:
        """Unwraps the command by stripping the first wrapper script.

        It looks for the first occurrence of `--` (surrounded by spaces)
        and returns everything after it.
        """
        tokens = shlex.split(self._str)

        try:
            idx = tokens.index("--")
        except ValueError:
            return None

        inner_tokens = tokens[idx + 1 :]
        if not inner_tokens:
            return ShellCommand("")

        return ShellCommand(inner_tokens)

    def parse(self) -> "ParsedShellCommand":
        """Parses the command into env vars, tool, and args.

        This uses a heuristic where the first token that doesn't contain '=' is
        considered the tool. Everything before it is environment variables.

        If the tool is a python interpreter, we heuristically look for the first
        argument ending in .py and treat that as the tool.
        """
        tokens = shlex.split(self._str)

        env_vars: dict[str, str] = {}
        tool: str | None = None
        args: list[str] = []
        for i, token in enumerate(tokens):
            varname, sep, value = token.partition("=")
            if sep == "=":
                env_vars[varname] = value
            else:
                tool = tokens[i]
                args = tokens[i + 1 :]
                break

        if not tool:
            return ParsedShellCommand(env_vars, "", [])

        # Heuristic: if tool is python, look for the actual script.
        base = os.path.basename(tool)
        if (
            base == "python"
            or base.startswith("python3")
            or base.startswith("fuchsia-vendored-python")
        ):
            for i, arg in enumerate(args):
                if arg.endswith(".py"):
                    tool = arg
                    args = args[i + 1 :]
                    break

        return ParsedShellCommand(env_vars, tool, args)


def find_command_with_tool(
    commands: list["ShellCommand"], tool: str
) -> T.Optional["ShellCommand"]:
    """
    Find the first command in the list that uses the given tool.
    Matches the basename of the tool.

    Args:
        commands: List of shell commands to search through.
        tool: Name of the tool to look for.

    Returns:
        The first command that uses the given tool, or None if not found.
    """
    for command in commands:
        current_command: ShellCommand | None = command
        while current_command:
            if os.path.basename(current_command.parse().tool) == tool:
                return current_command
            current_command = current_command.unwrap()
    return None


@dataclasses.dataclass
class ParsedShellCommand:
    """Represents a parsed shell command."""

    env_vars: dict[str, str]
    tool: str
    args: list[str]
