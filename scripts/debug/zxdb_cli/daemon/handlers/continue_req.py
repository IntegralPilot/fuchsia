# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from pydap.models import ContinueArguments
from shared.protocol.base import Response
from shared.protocol.continue_request import (
    COMMAND_NAME,
    ContinueRequest,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, req: ContinueRequest) -> Response[Any]:
    if not daemon.zxdb_writer:
        return Response(
            success=False, message="Not connected to zxdb DAP server"
        )

    args = ContinueArguments(
        threadId=req.thread_id, singleThread=req.single_thread
    )

    try:
        resp = await daemon.dap_client.continue_thread(args)
        all_threads_continued = resp.body.all_threads_continued in (None, True)
        daemon.update_resumed_threads(
            req.thread_id, single_thread=not all_threads_continued
        )
        return Response(success=True, body=resp.dump_dap())
    except Exception as e:
        return Response(success=False, message=f"Failed to continue: {e}")
