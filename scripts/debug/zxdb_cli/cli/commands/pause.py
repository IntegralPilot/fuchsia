# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any

from cli.commands.base import BaseCommand
from shared.protocol.pause import COMMAND_NAME


class Command(BaseCommand):
    COMMAND_NAME = COMMAND_NAME

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        pause_parser = subparsers.add_parser(
            Command.COMMAND_NAME, help="Interrupt execution"
        )
        group = pause_parser.add_mutually_exclusive_group(required=True)
        group.add_argument(
            "-t",
            "--thread-id",
            type=int,
            default=None,
            help="Thread ID to pause",
        )
        group.add_argument(
            "-p",
            "--pid",
            type=int,
            default=None,
            help="Process ID to pause. All threads in the process will be paused.",
        )
