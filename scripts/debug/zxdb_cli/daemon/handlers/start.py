# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING, Any

import package_server
from daemon.constants import DEFAULT_DAP_PORT, UDS_PATH
from shared.protocol.base import Response
from shared.protocol.start import (
    COMMAND_NAME,
    StartRequest,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, req: StartRequest) -> Response[Any]:
    if resp := daemon._check_already_running(req):
        return resp

    if req.port is not None:
        daemon.port = req.port
    elif daemon.port is None:
        daemon.port = DEFAULT_DAP_PORT
    daemon.connect_to_existing = req.connect

    startup_success = False
    try:
        if not daemon.connect_to_existing:
            if err_resp := await daemon._start_dap_server():
                return err_resp

        # Now connect to the DAP server
        connected = await daemon._connect_to_dap()
        if not connected:
            return Response(
                success=False, message="Failed to connect to DAP server"
            )

        startup_success = True
        return Response(success=True, body={"uds_path": str(UDS_PATH)})
    finally:
        if not startup_success:
            for task in daemon.background_tasks:
                task.cancel()
            daemon.background_tasks.clear()
            if daemon.zxdb_writer:
                daemon.zxdb_writer.close()
                try:
                    await daemon.zxdb_writer.wait_closed()
                except Exception:
                    pass
                daemon.zxdb_writer = None
            if daemon.dap_proc:
                daemon.dap_proc.terminate()
                daemon.dap_proc = None
            if daemon.package_server_proc:
                daemon.package_server_proc.terminate()
                await daemon.package_server_proc.wait()
                daemon.package_server_proc = None

                if daemon.repo_name:
                    await package_server.stop(daemon.repo_name)
                    daemon.repo_name = None
