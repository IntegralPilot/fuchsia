# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
from typing import Any

from cli.commands.base import BaseCommand
from shared.protocol.finish import COMMAND_NAME


class Command(BaseCommand):
    COMMAND_NAME = COMMAND_NAME
    ALIASES = ["step-out", "step_out", "stepOut", "stepout"]

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        parser = subparsers.add_parser(
            Command.COMMAND_NAME,
            aliases=Command.ALIASES,
            help="Finish execution of current function (step out)",
        )
        parser.add_argument("thread_id", type=int, help="Thread ID to finish")
        # TODO(https://fxbug.dev/542494515): Support single_thread option.
        parser.add_argument(
            "--single-thread",
            action="store_true",
            default=None,
            help=argparse.SUPPRESS,
        )
