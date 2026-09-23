# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import sys
from typing import Any, Final

from cli.commands.base import BaseCommand
from daemon_manager.manager import UDS_PATH, DaemonManager, DaemonManagerError
from shared.protocol.start import COMMAND_NAME

DAEMON_STARTUP_TIMEOUT_SECS: Final[float] = 30.0


class Command(BaseCommand):
    COMMAND_NAME = COMMAND_NAME

    @staticmethod
    def register_cli(subparsers: Any) -> None:
        start_parser = subparsers.add_parser(
            Command.COMMAND_NAME, help="Start the daemon"
        )
        start_parser.add_argument(
            "--port", type=int, help="Port for DAP server", default=None
        )
        start_parser.add_argument(
            "--connect",
            action="store_true",
            help="Connect to existing DAP server",
        )

    @staticmethod
    async def execute(args: argparse.Namespace) -> int:
        return await start_daemon(args.port, args.connect)


async def start_daemon(
    port: int | None, connect_to_existing: bool = False
) -> int:
    """Spawns the daemon process and waits for it to be ready."""
    manager = DaemonManager(
        socket_path=UDS_PATH,
        port=port,
        connect_to_existing=connect_to_existing,
        startup_timeout=DAEMON_STARTUP_TIMEOUT_SECS,
    )
    try:
        proc = await manager.start()
        if proc is None:
            print(
                json.dumps(
                    {
                        "success": True,
                        "message": "Connected to existing daemon",
                    }
                )
            )
        else:
            print(
                json.dumps(
                    {
                        "success": True,
                        "message": "Daemon started successfully",
                    }
                )
            )
        return 0
    except DaemonManagerError as e:
        print(
            json.dumps(
                {
                    "success": False,
                    "message": str(e),
                }
            ),
            file=sys.stderr,
        )
        return 1
    except Exception as e:
        print(
            json.dumps(
                {
                    "success": False,
                    "message": f"Failed to start daemon: {e}",
                }
            ),
            file=sys.stderr,
        )
        return 1
