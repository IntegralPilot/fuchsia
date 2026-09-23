# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING

from pydap.models import StackTraceArguments
from shared.protocol.base import Response
from shared.protocol.stack_trace import (
    COMMAND_NAME,
    ProcessStackTraceResponse,
    StackFrame,
    StackTraceRequest,
    ThreadStackTraceResponse,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


def _get_frame_origin(frame: StackFrame) -> str | None:
    return frame.source.origin if frame.source is not None else None


def collapse_elided_frames(
    frames: list[StackFrame],
) -> list[StackFrame]:
    result = []
    i = 0
    n = len(frames)
    while i < n:
        frame = frames[i]
        if frame.presentation_hint == "subtle":
            start_i = i
            group_origin = _get_frame_origin(frame)
            while (
                i + 1 < n
                and frames[i + 1].presentation_hint == "subtle"
                and _get_frame_origin(frames[i + 1]) == group_origin
            ):
                i += 1
            end_i = i
            if start_i == end_i:
                result.append(frame)
            else:
                desc = group_origin or "subtle frames"
                collapsed_frame = frame.model_copy(
                    update={
                        "frame_index": start_i,
                        "name": f"{start_i}…{end_i} «{desc}» (-r expands)",
                    }
                )
                result.append(collapsed_frame)
        else:
            result.append(frame)
        i += 1
    return result


async def _fetch_thread_stack_trace(
    daemon: Daemon,
    thread_id: int,
    raw: bool = False,
) -> ThreadStackTraceResponse:
    await daemon.ensure_stopped(thread_id)
    stack_resp = await daemon.dap_client.stack_trace(
        StackTraceArguments(
            thread_id=thread_id,
        ),
    )
    frames: list[StackFrame] = []
    total_frames = 0
    if stack_resp.body:
        raw_frames = stack_resp.body.stack_frames
        total_frames = (
            stack_resp.body.total_frames
            if stack_resp.body.total_frames is not None
            else len(raw_frames)
        )
        for i, f in enumerate(raw_frames):
            frames.append(
                StackFrame(
                    frame_index=i,
                    name=f.name,
                    line=f.line,
                    column=f.column,
                    source=f.source,
                    presentation_hint=f.presentation_hint,
                )
            )
    if not raw:
        frames = collapse_elided_frames(frames)
    return ThreadStackTraceResponse(
        thread_id=thread_id,
        stack_frames=frames,
        total_frames=total_frames,
    )


async def handle(
    daemon: Daemon, req: StackTraceRequest
) -> Response[ThreadStackTraceResponse | ProcessStackTraceResponse]:
    if not daemon.zxdb_writer:
        return Response(
            success=False, message="Not connected to zxdb DAP server"
        )

    try:
        if req.thread_id is not None:
            thread_trace = await _fetch_thread_stack_trace(
                daemon, req.thread_id, raw=req.raw
            )
            return Response(
                success=True,
                body=thread_trace,
            )

        if req.pid is None:
            return Response(
                success=False,
                message="Must specify either thread_id or pid",
            )

        await daemon.ensure_process_stopped(req.pid)
        process = daemon.processes.get(req.pid)
        if not process or not process.threads:
            return Response(
                success=False,
                message=f"No threads found for process {req.pid}",
            )

        threads_traces: list[ThreadStackTraceResponse] = []
        for t in process.threads.values():
            try:
                t_trace = await _fetch_thread_stack_trace(
                    daemon, t.id, raw=req.raw
                )
                threads_traces.append(t_trace)
            except Exception:
                threads_traces.append(
                    ThreadStackTraceResponse(
                        thread_id=t.id,
                        stack_frames=[],
                        total_frames=0,
                    )
                )

        return Response(
            success=True,
            body=ProcessStackTraceResponse(
                process_id=req.pid,
                stacks=threads_traces,
            ),
        )
    except Exception as e:
        return Response(
            success=False, message=f"Failed to get stack trace: {e}"
        )
