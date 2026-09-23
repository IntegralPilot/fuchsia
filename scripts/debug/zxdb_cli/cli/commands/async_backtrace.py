# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any

from cli.commands.base import BaseCommand
from shared.protocol.async_backtrace import COMMAND_NAME


class Command(BaseCommand):
    COMMAND_NAME = COMMAND_NAME
    ALIASES = [
        "abt",
        "async_backtrace",
        "asyncBacktrace",
        "async-tasks",
        "async_tasks",
        "asyncTasks",
        "tasks",
    ]

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        parser = subparsers.add_parser(
            Command.COMMAND_NAME,
            aliases=Command.ALIASES,
            help="Get asynchronous backtrace (async task tree) for a process",
        )
        parser.add_argument(
            "-p",
            "--pid",
            type=int,
            default=None,
            help=(
                "Process ID (KOID) to get async backtrace for (optional if"
                " single process)."
            ),
        )
