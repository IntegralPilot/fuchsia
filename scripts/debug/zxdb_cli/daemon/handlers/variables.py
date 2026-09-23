# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from pydap.models import (
    ScopesArguments,
    StackTraceArguments,
    VariablesArguments,
)
from shared.protocol.base import Response
from shared.protocol.variables import (
    COMMAND_NAME,
    VariablesRequest,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, req: VariablesRequest) -> Response[Any]:
    if not daemon.zxdb_writer:
        return Response(
            success=False, message="Not connected to zxdb DAP server"
        )

    try:
        await daemon.ensure_stopped(req.thread_id)

        # Retrieve stacktrace
        stack_resp = await daemon.dap_client.stack_trace(
            StackTraceArguments(
                thread_id=req.thread_id,
            ),
        )

        if not stack_resp.body or not stack_resp.body.stack_frames:
            return Response(success=False, message="No stack frames found")

        if req.frame_index < 0 or req.frame_index >= len(
            stack_resp.body.stack_frames
        ):
            return Response(
                success=False,
                message=(
                    f"Frame index {req.frame_index} out of range (stack has"
                    f" {len(stack_resp.body.stack_frames)} frames)"
                ),
            )

        frame = stack_resp.body.stack_frames[req.frame_index]

        # Get scopes for that frame
        scopes_resp = await daemon.dap_client.scopes(
            ScopesArguments(frame_id=frame.id),
        )

        variables = []
        if scopes_resp.body and scopes_resp.body.scopes:
            for scope in scopes_resp.body.scopes:
                if scope.name in ("Locals", "Arguments"):
                    vars_resp = await daemon.dap_client.variables(
                        VariablesArguments(
                            variables_reference=scope.variables_reference
                        ),
                    )
                    if vars_resp.body and vars_resp.body.variables:
                        for v in vars_resp.body.variables:
                            variables.append(
                                {
                                    "name": v.name,
                                    "value": v.value,
                                    "type": v.type,
                                }
                            )

        return Response(
            success=True,
            body={"variables": variables},
        )
    except Exception as e:
        return Response(success=False, message=f"Failed to get variables: {e}")
