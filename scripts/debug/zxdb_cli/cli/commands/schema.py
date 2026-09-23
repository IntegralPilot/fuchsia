# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
from typing import Any

from cli.commands.base import BaseCommand
from shared.protocol import get_schema


class Command(BaseCommand):
    @staticmethod
    def register_cli(subparsers: Any) -> None:
        schema_parser = subparsers.add_parser(
            "schema", help="Print the JSON schema of the protocol"
        )
        schema_parser.add_argument(
            "--indent", type=int, default=2, help="JSON indentation level"
        )

    @staticmethod
    async def execute(args: argparse.Namespace) -> int:
        print(json.dumps(get_schema(), indent=args.indent))
        return 0
