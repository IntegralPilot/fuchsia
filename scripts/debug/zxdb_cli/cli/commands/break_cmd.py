# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import sys
from typing import Any

from cli.commands.base import BaseCommand
from shared.protocol.break_request import COMMAND_NAME


def resolve_path(filepath: str) -> str | None:
    """Resolve a file path to an absolute path.

    Paths must be fully qualified from the workspace root (FUCHSIA_DIR), or
    be an absolute path, or exist relative to the current working directory.
    """
    abs_path = os.path.abspath(filepath)
    if os.path.isfile(abs_path):
        return abs_path

    if os.path.isabs(filepath):
        return None

    fuchsia_dir = os.environ.get("FUCHSIA_DIR")
    if not fuchsia_dir:
        return None

    fuchsia_path = os.path.abspath(os.path.join(fuchsia_dir, filepath))
    if os.path.isfile(fuchsia_path):
        return fuchsia_path

    return None


class Command(BaseCommand):
    COMMAND_NAME = COMMAND_NAME
    ALIASES = [
        "breakpoint",
        "b",
        "setBreakpoints",
        "set-breakpoints",
        "set_breakpoints",
    ]

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        break_parser = subparsers.add_parser(
            Command.COMMAND_NAME,
            aliases=Command.ALIASES,
            help="Set or delete a breakpoint",
            description=(
                "Set or delete a breakpoint at the specified file and line. "
                "Paths must be fully qualified from the workspace root, "
                "an absolute path, or relative to the current working directory. "
                "To delete a breakpoint, the file and line specification "
                "must match an existing breakpoint. "
                "To view currently installed breakpoints, use the get-state command."
            ),
        )
        break_parser.add_argument(
            "file_line",
            type=str,
            help="File and line number (fully qualified from workspace root, absolute, or relative to CWD), e.g. 'main.cc:23' or 'src/foo.rs:10'",
        )
        break_parser.add_argument(
            "-d",
            "--delete",
            action="store_true",
            help=(
                "Delete the breakpoint at the specified file and line "
                "instead of adding it"
            ),
        )

    @staticmethod
    def _parse_and_resolve_file_line(
        file_line: str,
    ) -> tuple[str, int] | None:
        """Parses '<file>:<line>' and resolves the file path against the workspace."""
        if ":" not in file_line:
            print(
                f"Error: Invalid format '{file_line}'. Expected '<file>:<line>'.",
                file=sys.stderr,
            )
            return None

        parts = file_line.rsplit(":", 1)
        file_part = parts[0]
        line_part = parts[1]

        try:
            line = int(line_part)
            if line <= 0:
                raise ValueError()
        except ValueError:
            print(
                f"Error: Invalid line number '{line_part}'. Must be a positive integer.",
                file=sys.stderr,
            )
            return None

        resolved_file = resolve_path(file_part)
        if not resolved_file:
            print(
                f"Error: Could not resolve file path '{file_part}'. "
                "Paths must be fully qualified from the workspace root, "
                "an absolute path, or relative to the current working directory. "
                "Ensure FUCHSIA_DIR is set or run from the workspace root.",
                file=sys.stderr,
            )
            return None

        return resolved_file, line

    @staticmethod
    async def execute(args: argparse.Namespace) -> int | None:
        # In JSON mode (--json), cli.py invokes execute() with a namespace converted from
        # an already-deserialized BreakRequest (which has 'file' and 'line', but no 'file_line').
        # In CLI mode, argparse populates 'file_line', which we must split and resolve.
        if hasattr(args, "file") and hasattr(args, "line"):
            return None

        parsed = Command._parse_and_resolve_file_line(args.file_line)
        if parsed is None:
            return 1

        args.file, args.line = parsed
        # Remove CLI-only file_line attribute so the namespace matches the JSON BreakRequest structure (file and line).
        if hasattr(args, "file_line"):
            delattr(args, "file_line")
        return None
