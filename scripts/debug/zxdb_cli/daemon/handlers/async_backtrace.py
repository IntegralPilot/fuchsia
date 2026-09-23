# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING

from shared.protocol import Response
from shared.protocol.async_backtrace import (
    COMMAND_NAME,
    AsyncBacktraceRequest,
    AsyncBacktraceResponse,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(
    daemon: Daemon, req: AsyncBacktraceRequest
) -> Response[AsyncBacktraceResponse]:
    if not daemon.zxdb_writer:
        return Response(
            success=False, message="Not connected to zxdb DAP server"
        )

    try:
        target_proc = None
        if req.pid is not None:
            target_proc = daemon.processes.get(req.pid)
            if not target_proc:
                return Response(
                    success=False,
                    message=f"Process {req.pid} not found",
                )
        else:
            if len(daemon.processes) == 1:
                target_proc = next(iter(daemon.processes.values()))
            elif len(daemon.processes) == 0:
                return Response(
                    success=False,
                    message="No active processes found",
                )
            else:
                return Response(
                    success=False,
                    message="Multiple processes active; please specify a PID with --pid / -p",
                )

        if target_proc.async_backtrace is None:
            return Response(
                success=False,
                message=f"No async backtrace cached for process {target_proc.id}",
            )

        return Response(
            success=True,
            body=AsyncBacktraceResponse(
                process_id=target_proc.id,
                tasks=target_proc.async_backtrace,
            ),
        )
    except Exception as e:
        return Response(
            success=False,
            message=f"Failed to get async backtrace: {e}",
        )
