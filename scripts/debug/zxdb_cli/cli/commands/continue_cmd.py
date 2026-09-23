# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any

from cli.commands.base import BaseCommand
from shared.protocol.continue_request import COMMAND_NAME


class Command(BaseCommand):
    COMMAND_NAME = COMMAND_NAME

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        continue_parser = subparsers.add_parser(
            Command.COMMAND_NAME, help="Resume execution"
        )
        continue_parser.add_argument(
            "thread_id", type=int, help="Thread ID to resume"
        )

        # Note that unlike other thread control commands (step_in, step_out, etc), this is respected
        # in zxdb's backend, and so therefore doesn't need anything additional here to reflect that
        # behavior in the rest of the system.
        continue_parser.add_argument(
            "--single-thread",
            action="store_true",
            default=None,
            help="Resume only the specified thread",
        )
