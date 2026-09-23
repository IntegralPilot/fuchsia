# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from pydap.models import StepInArguments
from shared.protocol.base import Response
from shared.protocol.step_in import (
    COMMAND_NAME,
    StepInRequest,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, req: StepInRequest) -> Response[Any]:
    """Handles a step_in (step into) command request.

    Args:
        daemon: Daemon instance holding active DAP client session.
        req: StepInRequest containing parameters.

    Returns:
        Response object with success status and optional body or error message.
    """
    if not daemon.zxdb_writer:
        return Response(
            success=False, message="Not connected to zxdb DAP server"
        )

    # zxdb currently ignores this argument today, but behaves as if it were set to True in all
    # cases. For right now we set that to be our default if not specified from the command line as
    # well so that when zxdb starts paying attention to this argument we preserve the existing
    # default behavior. See https://fxbug.dev/542494515.
    single_thread = req.single_thread if req.single_thread is not None else True
    args = StepInArguments(
        thread_id=req.thread_id,
        single_thread=single_thread,
        target_id=req.target_id,
        granularity=req.granularity,
    )

    try:
        resp = await daemon.dap_client.step_in(args)
        daemon.update_resumed_threads(
            req.thread_id, single_thread=single_thread
        )
        return Response(success=True, body=resp.dump_dap())
    except Exception as e:
        return Response(success=False, message=f"Failed to step in: {e}")
