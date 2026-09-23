# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from pydap.dap_types import Source, SourceBreakpoint
from pydap.models import SetBreakpointsArguments
from shared.protocol.base import Response
from shared.protocol.break_request import (
    COMMAND_NAME,
    BreakRequest,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, req: BreakRequest) -> Response[Any]:
    if not daemon.zxdb_writer:
        return Response(
            success=False, message="Not connected to zxdb DAP server"
        )

    # TODO(https://fxbug.dev/530560621): Use a per-file lock wrapped around the
    # breakpoint set to prevent race conditions during concurrent updates.
    lines = set(daemon.active_breakpoints.get(req.file, set()))

    if req.delete:
        if req.line in lines:
            lines.remove(req.line)
    else:
        lines.add(req.line)

    try:
        args = SetBreakpointsArguments(
            source=Source(path=req.file),
            breakpoints=[SourceBreakpoint(line=line) for line in sorted(lines)],
        )
        resp = await daemon.dap_client.set_breakpoints(args)
        if resp.success:
            daemon.active_breakpoints[req.file] = lines
            return Response(success=True, body=resp.body.dump_dap())
        else:
            return Response(success=False, message=resp.message)
    except Exception as e:
        return Response(
            success=False, message=f"Failed to set breakpoints: {e}"
        )
