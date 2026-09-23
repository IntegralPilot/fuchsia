# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from pydantic import BaseModel, ConfigDict, Field
from shared.protocol.base import BaseRequest
from zxdb_dap import AsyncTaskNode

COMMAND_NAME: Final = "async-backtrace"


class AsyncBacktraceResponse(BaseModel):
    """Response containing cached async tasks for a process."""

    model_config = ConfigDict(populate_by_name=True)

    process_id: int
    tasks: list[AsyncTaskNode] = Field(default_factory=list)


class AsyncBacktraceRequest(BaseRequest[AsyncBacktraceResponse]):
    """Request asynchronous backtrace (cached async task tree) for a process."""

    command: Literal["async-backtrace"] = COMMAND_NAME
    pid: int | None = None
    response_type: ClassVar[Any] = AsyncBacktraceResponse
