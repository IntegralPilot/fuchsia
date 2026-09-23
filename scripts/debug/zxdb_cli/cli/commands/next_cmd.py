# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
from typing import Any

from cli.commands.base import BaseCommand
from shared.protocol.next_request import COMMAND_NAME


class Command(BaseCommand):
    """CLI command implementation for stepping over execution (next)."""

    COMMAND_NAME = COMMAND_NAME
    ALIASES = ["n", "step-over", "step_over", "stepOver", "stepover"]

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        parser = subparsers.add_parser(
            Command.COMMAND_NAME,
            aliases=Command.ALIASES,
            help="Step over execution to the next line (next)",
        )
        parser.add_argument(
            "thread_id", type=int, help="Thread ID to step over"
        )
        # TODO(https://fxbug.dev/542494515): Support single_thread option.
        parser.add_argument(
            "--single-thread",
            action="store_true",
            default=None,
            help=argparse.SUPPRESS,
        )
        # TODO(https://fxbug.dev/542495451): Support SteppingGranularity.
        parser.add_argument(
            "--granularity",
            choices=["statement", "line", "instruction"],
            default=None,
            help=argparse.SUPPRESS,
        )
