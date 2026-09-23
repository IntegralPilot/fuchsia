# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
from typing import Any, ClassVar


class BaseCommand:
    COMMAND_NAME: ClassVar[str] = ""
    ALIASES: ClassVar[list[str]] = []

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        raise NotImplementedError()

    @staticmethod
    async def execute(args: argparse.Namespace) -> int | None:
        # Default fallback: return None to indicate no custom execution logic.
        return None
