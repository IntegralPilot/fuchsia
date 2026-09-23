# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any

from cli.commands.base import BaseCommand
from shared.protocol.variables import COMMAND_NAME


class Command(BaseCommand):
    COMMAND_NAME = COMMAND_NAME
    ALIASES = ["locals"]

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        variables_parser = subparsers.add_parser(
            Command.COMMAND_NAME,
            aliases=Command.ALIASES,
            help="Get variables of a stack frame.",
            description=(
                "Get variables (locals and arguments) of a stack frame.\n\n"
                "If the target thread is running, it will be automatically paused to "
                "retrieve the stack trace. Note that the stack frames (and their indices) "
                "may have changed if the thread had to be paused."
            ),
        )
        variables_parser.add_argument(
            "-t",
            "--thread-id",
            type=int,
            required=True,
            help="Thread ID to query",
        )
        variables_parser.add_argument(
            "--frame-index",
            type=int,
            default=0,
            help="Frame index within the stacktrace (defaults to 0)",
        )
