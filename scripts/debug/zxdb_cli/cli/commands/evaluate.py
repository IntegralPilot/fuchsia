# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
from typing import Any

from cli.commands.base import BaseCommand
from shared.protocol.evaluate import COMMAND_NAME


class Command(BaseCommand):
    COMMAND_NAME = COMMAND_NAME
    ALIASES = ["print"]

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        evaluate_parser = subparsers.add_parser(
            Command.COMMAND_NAME,
            aliases=Command.ALIASES,
            help="Evaluate an expression in a stack frame.",
            description=(
                "Evaluate an expression in the context of a stack frame."
            ),
        )
        evaluate_parser.add_argument(
            "-t",
            "--thread-id",
            type=int,
            required=True,
            help="Thread ID to query",
        )
        evaluate_parser.add_argument(
            "--frame-index",
            type=int,
            default=0,
            help="Frame index within the stacktrace (defaults to 0)",
        )
        evaluate_parser.add_argument(
            "--start",
            type=int,
            default=0,
            help="Index of the first child variable to retrieve (defaults to 0)",
        )
        evaluate_parser.add_argument(
            "--count",
            type=int,
            default=50,
            help="Number of child variables to retrieve (defaults to 50)",
        )
        evaluate_parser.add_argument(
            "expression",
            nargs="+",
            help="Expression to evaluate",
        )

    @staticmethod
    async def execute(args: argparse.Namespace) -> int | None:
        # TODO(https://fxbug.dev/527992704): Handle expression evaluation vs generic console commands
        # appropriately once zxdb evaluate handler is restricted to expressions only.
        if isinstance(args.expression, list):
            args.expression = " ".join(args.expression)
        return None
