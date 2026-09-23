# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from shared.protocol.base import Response
from shared.protocol.detach import (
    COMMAND_NAME,
    DetachRequest,
)
from zxdb_dap import ZxdbDetachArguments

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, req: DetachRequest) -> Response[Any]:
    if not daemon.zxdb_writer:
        return Response(
            success=False, message="Not connected to zxdb DAP server"
        )

    try:
        if req.all:
            args = ZxdbDetachArguments(all=True)
        else:
            args = ZxdbDetachArguments(pid=req.pid)
        resp = await daemon.dap_client.zxdb_detach(args)
        if not resp.get("success"):
            return Response(
                success=False,
                message=resp.get("message", "Failed to detach from process"),
            )
        if req.all:
            daemon.clear_all_processes()
        elif req.pid is not None:
            daemon.clear_process(req.pid)

        # Synthesize and enqueue detached event
        await daemon.event_queue.put(
            {"event": "detached", "body": {"pid": req.pid, "all": req.all}}
        )
        return Response(success=True)
    except Exception as e:
        return Response(success=False, message=f"Failed to detach: {e}")
