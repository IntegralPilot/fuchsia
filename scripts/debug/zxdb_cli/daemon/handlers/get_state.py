# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING

from shared.protocol.base import Response
from shared.protocol.get_state import (
    COMMAND_NAME,
    GetStateRequest,
    GetStateResponse,
    ThreadInfo,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(
    daemon: Daemon, _req: GetStateRequest
) -> Response[GetStateResponse]:
    """Queries the debug adapter for the current threads, active
    processes, and active breakpoints.

    Returns:
        A Response containing a GetStateResponse body mapping active
        processes, threads, and breakpoints.
    """
    if not daemon.zxdb_writer:
        return Response(
            success=False, message="Not connected to zxdb DAP server"
        )
    try:
        # TODO(https://fxbug.dev/552813275): Wrap zxdb requests with
        # capability.enable(zxdb) before calling zxdb_threads.
        threads_resp = await daemon.dap_client.zxdb_threads()
        threads = []
        # Defensive check to ensure zxdb DAP server successfully returned a
        # valid threads list.
        if threads_resp.body and threads_resp.body.threads is not None:
            daemon.update_thread_cache(threads_resp.body.threads)
            for t in threads_resp.body.threads:
                threads.append(ThreadInfo(id=t.id, name=t.name))

        breakpoints = {
            file: sorted(list(lines))
            for file, lines in daemon.active_breakpoints.items()
            if lines
        }
        state_resp = GetStateResponse(
            threads=threads,
            processes={p.id: p.name for p in daemon.processes.values()},
            breakpoints=breakpoints or None,
        )
        return Response(
            success=True,
            body=state_resp,
        )
    except Exception as e:
        return Response(success=False, message=f"Failed to get threads: {e}")
