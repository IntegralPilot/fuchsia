# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
from typing import Any

from cli.commands.base import BaseCommand
from shared.protocol.step_in import COMMAND_NAME


class Command(BaseCommand):
    """CLI command implementation for stepping into execution (step_in)."""

    COMMAND_NAME = COMMAND_NAME
    ALIASES = [
        "step_in",
        "stepIn",
        "stepin",
        "step-into",
        "step_into",
        "stepinto",
        "step",
        "s",
    ]

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        parser = subparsers.add_parser(
            Command.COMMAND_NAME,
            aliases=Command.ALIASES,
            help="Step into execution of current function or line (step in)",
        )
        parser.add_argument(
            "thread_id", type=int, help="Thread ID to step into"
        )
        # TODO(https://fxbug.dev/542494515): Support single_thread option.
        parser.add_argument(
            "--single-thread",
            action="store_true",
            default=None,
            help=argparse.SUPPRESS,
        )
        # TODO(https://fxbug.dev/532508554): Support target_id option.
        parser.add_argument(
            "--target-id",
            type=int,
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
