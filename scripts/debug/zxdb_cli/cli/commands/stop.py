# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import sys
from typing import Any

from cli.commands.base import BaseCommand
from daemon_manager.manager import UDS_PATH, DaemonManager
from shared.protocol.stop import COMMAND_NAME


class Command(BaseCommand):
    COMMAND_NAME = COMMAND_NAME

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        subparsers.add_parser(Command.COMMAND_NAME, help="Stop the daemon")

    @staticmethod
    async def execute(args: argparse.Namespace) -> int:
        return await stop_daemon()


async def stop_daemon() -> int:
    """Stops the daemon using DaemonManager.

    Note: We do not wait for the process directly because the CLI is a
    short-lived invocation and we do not persist the DaemonManager's process
    handle across commands. Instead, `manager.stop()` gracefully requests the
    daemon stop via UDS, which drains the socket connection before shutdown.
    """
    manager = DaemonManager(socket_path=UDS_PATH)
    try:
        await manager.stop()
        return 0
    except Exception as e:
        print(f"Error stopping daemon: {e}", file=sys.stderr)
        return 1
