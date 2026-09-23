# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from pydantic import BaseModel
from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "get-state"


class ThreadInfo(BaseModel):
    """Information about a single thread."""

    id: int
    name: str


class GetStateResponse(BaseModel):
    """Response for get-state command containing thread list, active
    processes, and active breakpoints.
    """

    threads: list[ThreadInfo]
    processes: dict[int, str] | None = None
    breakpoints: dict[str, list[int]] | None = None


class GetStateRequest(BaseRequest[GetStateResponse]):
    """Request current state of threads."""

    command: Literal["get-state"] = COMMAND_NAME
    response_type: ClassVar[Any] = GetStateResponse
