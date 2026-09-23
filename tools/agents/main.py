#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Main entry point for `fx agents` CLI."""

from __future__ import annotations

import argparse
import sys
from collections.abc import Sequence

from agents.commands import setup


def create_parser() -> argparse.ArgumentParser:
    """Create the top-level argument parser for `fx agents`."""
    parser = argparse.ArgumentParser(
        prog="fx agents",
        description="Manage AI coding agent configuration, permissions, and developer workflows.",
    )
    subparsers = parser.add_subparsers(
        dest="subcommand",
        metavar="COMMAND",
        help="Subcommand to execute.",
    )
    setup.register_subcommand(subparsers)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Main CLI execution handler."""
    parser = create_parser()
    if argv is None:
        argv = sys.argv[1:]

    args = parser.parse_args(argv)
    func = getattr(args, "func", None)
    if func is None:
        parser.print_help(sys.stderr)
        return 1

    return func(args)


if __name__ == "__main__":
    sys.exit(main())
