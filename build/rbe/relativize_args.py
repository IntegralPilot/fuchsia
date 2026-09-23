#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Relativize shell command arguments to be relative.

This script helps convert commands with absolute paths to equivalent
commands using relative paths.  Paths are transformed blindly without
verifying existence or validity.
This helps reclient gather arguments under a common --exec_root directory.
Note, however, that this script is unaware of exec_root; it is the
responsibility of the invoker to make sure all path arguments fall
under a common exec_root.
"""

import argparse
import os
import subprocess
import sys
from collections.abc import Callable, Sequence
from pathlib import Path

_SCRIPT_BASENAME = os.path.basename(__file__)


def msg(text: str) -> None:
    print(f"[{_SCRIPT_BASENAME}] {text}")


def split_transform_join(
    token: str, sep: str, transform: Callable[[str], str]
) -> str:
    return sep.join(transform(x) for x in token.split(sep))


def lexically_rewrite_token(token: str, transform: Callable[[str], str]) -> str:
    """Lexically replaces substrings between delimiters.

    This is useful for transforming substrings of text.

    This can transform "--foo=bar,baz" into
    f("--foo") + "=" + f("bar") + "," + f("baz")

    Args:
      token: text to transform, like a shell token.
      transform: text transformation.

    Returns:
      text with substrings transformed.
    """

    def inner_transform(text: str) -> str:
        return split_transform_join(text, ",", transform)

    return split_transform_join(token, "=", inner_transform)


def greatest_path_parent(p: Path) -> Path:
    assert p.is_absolute()
    return Path(*p.parts[:2])  # keep the leading '/' and the first component


def relativize_path(arg: str, start: Path) -> str:
    """Convert a path or path substring to relative.

    Args:
      arg: string that is a path or may contain a path.
      start: result paths are relative to this directory (absolute).

    Returns:
      possibly transformed arg with relative paths.
    """
    start_abs = start.absolute()
    # Handle known compiler flags like -I/abs/path, -L/abs/path
    # Such flags are fused to their arguments without a delimiter.
    for flag in ("-I", "-L", "-isystem"):
        if arg.startswith(flag):
            suffix = arg[len(flag) :]
            return flag + relativize_path(suffix, start=start_abs)

    try_path = Path(arg)
    # Windows-style flags look like absolute paths, e.g. /Foo
    # so we leave those alone by checking for existence.
    # Only check the existence of the greatest parent, because some paths
    # may refer to outputs that do not exist yet.
    if try_path.is_absolute() and greatest_path_parent(try_path).exists():
        # Can't use Path.relative_to() because arguments
        # aren't guaranteed to be subdir of the other.
        return os.path.relpath(arg, start=start_abs)

    return arg


def relativize_command(
    command: Sequence[str], working_dir: Path
) -> Sequence[str]:
    """Transform a command to use relative paths.

    Args:
      command: the command to transform, sequence of shell tokens.
      working_dir: result paths are relative to this (absolute).

    Returns:
      command using relative paths
    """
    relativized_command = []
    # Subprocess calls do not work for commands that start with VAR=VALUE
    # environment variables, which is remedied by prefixing with 'env'.
    if command and "=" in command[0]:
        relativized_command += ["/usr/bin/env"]

    relativized_command += [
        lexically_rewrite_token(tok, lambda x: relativize_path(x, working_dir))
        for tok in command
    ]

    return relativized_command


def main_arg_parser() -> argparse.ArgumentParser:
    """Construct the argument parser, called by main()."""
    parser = argparse.ArgumentParser(
        description="Transforms a command to use relative paths.",
        argument_default=[],
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        default=False,
        help="Print information rewritten command.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        default=False,
        help="Show transformed command and exit.",
    )
    parser.add_argument(
        "--cwd",
        type=Path,
        default=Path(os.curdir),
        help="Override the current working dir for relative paths.",
    )
    parser.add_argument(
        "--disable",
        action="store_false",
        dest="enable",
        default=True,
        help="If disabled, run the original command as-is.",
    )

    # Positional args are the command and arguments to run.
    parser.add_argument("command", nargs="*", help="The command to run")
    return parser


def main(argv: Sequence[str]) -> int:
    parser = main_arg_parser()
    args = parser.parse_args(argv)

    command = args.command
    relativized_command = relativize_command(
        command=command, working_dir=args.cwd
    )

    cmd_str = " ".join(relativized_command)
    if args.verbose or args.dry_run:
        msg(f"Relativized command: {cmd_str}")

    if args.dry_run:
        return 0

    if not args.enable:
        return subprocess.call(command)

    exit_code = subprocess.call(relativized_command)
    if exit_code != 0:
        msg(f"*** Relativized command failed (exit={exit_code}): {cmd_str}")
    return exit_code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
