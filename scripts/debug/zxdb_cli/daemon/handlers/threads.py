# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from shared.protocol.base import Response
from shared.protocol.threads import (
    COMMAND_NAME,
    ThreadsRequest,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, _req: ThreadsRequest) -> Response[Any]:
    if not daemon.zxdb_writer:
        return Response(
            success=False, message="Not connected to zxdb DAP server"
        )

    try:
        # TODO(https://fxbug.dev/552813275): Wrap zxdb requests with
        # capability.enable(zxdb) before calling zxdb_threads.
        resp = await daemon.dap_client.zxdb_threads()
        if resp.body and resp.body.threads is not None:
            daemon.update_thread_cache(resp.body.threads)
        body = resp.body.model_dump() if resp.body else None
        return Response(
            success=True,
            body=body,
        )
    except Exception as e:
        return Response(success=False, message=f"Failed to get threads: {e}")
