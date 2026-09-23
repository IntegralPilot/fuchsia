#!/usr/bin/env fuchsia-vendored-python
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Launch a command and generate a Ninja depfile for it from an input hermetic inputs file."""

import argparse
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path


def depfile_quote(path: str) -> str:
    """Quote a path properly for depfiles, if necessary.

    shlex.quote() does not work because paths with spaces
    are simply encased in single-quotes, while the Ninja
    depfile parser only supports escaping single chars
    (e.g. ' ' -> '\ ').

    Args:
       path: input file path.
    Returns:
       The input file path with proper quoting to be included
       directly in a depfile.
    """
    return path.replace("\\", "\\\\").replace(" ", "\\ ")


def depfile_list(paths: Sequence[str]) -> str:
    """Create a string that contains a list of paths, quoted for depfiles.

    Args:
       paths: List of input path strings
    Returns:
       A string containing a space-separated list of paths, properly
       quoted to be written into a depfile.
    """
    return " ".join(depfile_quote(p) for p in paths)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--hermetic-inputs-file",
        type=argparse.FileType("r"),
        required=True,
        help="Path to input hermetic inputs file",
    )
    parser.add_argument(
        "--outputs",
        required=True,
        nargs="*",
        help="Action outputs, to be listed in generated depfile",
    )
    parser.add_argument(
        "--depfile", required=True, help="Path to output depfile"
    )
    parser.add_argument(
        "command", nargs=argparse.REMAINDER, help="Action command"
    )

    args = parser.parse_args()

    # Read implicit inputs from file.
    implicit_inputs = [
        l.rstrip() for l in args.hermetic_inputs_file.readlines()
    ]

    # Read command, and remove initial -- if it is found.
    cmd_args = args.command
    if cmd_args[0] == "--":
        cmd_args = cmd_args[1:]

    # If command is a Python script, invoke it through the same interpreter.
    tool = cmd_args[0]
    if tool.endswith((".py", ".pyz")):
        cmd_args = [sys.executable, "-S"] + cmd_args

    # Run the command.
    try:
        subprocess.check_call(cmd_args)
    except subprocess.CalledProcessError as exc:
        # Simply forward the exit code instead of raising an exception to avoid
        # polluting every build error message with a generic stack trace from
        # this script.
        return exc.returncode

    # Generate the depfile.
    depfile = Path(args.depfile)
    depfile.parent.mkdir(exist_ok=True, parents=True)
    with depfile.open("w") as f:
        f.write(
            "%s: %s\n"
            % (depfile_list(args.outputs), depfile_list(implicit_inputs))
        )

    return 0


if __name__ == "__main__":
    sys.exit(main())
