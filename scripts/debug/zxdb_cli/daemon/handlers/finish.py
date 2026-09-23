# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from pydap.models import StepOutArguments
from shared.protocol.base import Response
from shared.protocol.finish import (
    COMMAND_NAME,
    FinishRequest,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, req: FinishRequest) -> Response[Any]:
    if not daemon.zxdb_writer:
        return Response(
            success=False, message="Not connected to zxdb DAP server"
        )

    # zxdb currently ignores this argument today, but behaves as if it were set to True in all
    # cases. For right now we set that to be our default if not specified from the command line as
    # well so that when zxdb starts paying attention to this argument we preserve the existing
    # default behavior. See https://fxbug.dev/542494515.
    single_thread = req.single_thread if req.single_thread is not None else True
    args = StepOutArguments(
        thread_id=req.thread_id, single_thread=single_thread
    )

    try:
        resp = await daemon.dap_client.step_out(args)
        daemon.update_resumed_threads(
            req.thread_id, single_thread=single_thread
        )
        return Response(success=True, body=resp.dump_dap())
    except Exception as e:
        return Response(success=False, message=f"Failed to finish frame: {e}")
