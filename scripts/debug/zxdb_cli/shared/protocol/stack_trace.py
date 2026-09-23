# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from pydantic import BaseModel, model_validator
from pydap.dap_types import Source
from shared.protocol.base import BaseRequest


class StackFrame(BaseModel):
    """A stack frame representation exposing frame_index instead of DAP frameId."""

    frame_index: int
    name: str
    line: int
    column: int
    source: Source | None = None
    presentation_hint: str | None = None


class ThreadStackTraceResponse(BaseModel):
    """Stack trace for a single thread in a process."""

    thread_id: int
    stack_frames: list[StackFrame]
    total_frames: int


class ProcessStackTraceResponse(BaseModel):
    """Response containing stack traces for all threads in a process."""

    process_id: int
    stacks: list[ThreadStackTraceResponse]


COMMAND_NAME: Final = "stack-trace"


class StackTraceRequest(
    BaseRequest[ThreadStackTraceResponse | ProcessStackTraceResponse]
):
    """Request stack trace for a thread or all threads in a process."""

    command: Literal["stack-trace"] = COMMAND_NAME
    thread_id: int | None = None
    pid: int | None = None
    raw: bool = False
    response_type: ClassVar[Any] = (
        ThreadStackTraceResponse | ProcessStackTraceResponse
    )

    @model_validator(mode="after")
    def validate_targets(self) -> "StackTraceRequest":
        if self.thread_id is not None and self.pid is not None:
            raise ValueError("Cannot specify both thread_id and pid")
        if self.thread_id is None and self.pid is None:
            raise ValueError("Must specify either thread_id or pid")
        return self
