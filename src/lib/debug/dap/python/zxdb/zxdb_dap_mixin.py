# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, Protocol

from pydantic import Field, model_validator
from pydap.dap_types import DapBaseModel, Thread
from pydap.models import Response, StackTraceArguments, StackTraceResponse
from zxdb_dap.models import (
    ZxdbPauseArguments,
    ZxdbThreadsResponse,
)


class ZxdbStackTraceArguments(StackTraceArguments):
    """Arguments for zxdb `stackTrace` request.

    Attributes:
        remote_unwind: Force remote unwind on the target.
    """

    remote_unwind: bool | None = None


class ZxdbDetachArguments(DapBaseModel):
    """Arguments for `zxdb.Detach` request.

    Attributes:
        pid: Process ID to detach from.
        detach_all: Whether to detach from all processes.
    """

    pid: int | None = None
    detach_all: bool | None = Field(alias="all", default=None)

    @model_validator(mode="after")
    def validate_exclusive_args(self) -> "ZxdbDetachArguments":
        if self.detach_all and self.pid is not None:
            raise ValueError("Cannot specify pid when detach_all is True")
        if not self.detach_all and self.pid is None:
            raise ValueError("Must specify either pid or detach_all")
        return self


class ZxdbProcessArguments(DapBaseModel):
    """Arguments for `zxdb.Process` request.

    Attributes:
        pid: Optional process ID to query.
    """

    pid: int | None = None


class ZxdbProcessInfo(DapBaseModel):
    """Process info returned by `zxdb.Process` request.

    Attributes:
        id: Process ID (KOID).
        name: Process name.
        threads: List of threads associated with the process.
    """

    id: int
    name: str
    threads: list[Thread] = Field(default_factory=list)


class ZxdbProcessResponseBody(DapBaseModel):
    """Response body for `zxdb.Process` request."""

    processes: list[ZxdbProcessInfo] = Field(default_factory=list)


class ZxdbProcessResponse(Response):
    """Response for `zxdb.Process` request."""

    body: ZxdbProcessResponseBody = Field(
        default_factory=ZxdbProcessResponseBody
    )


class SupportsSendRequest(Protocol):
    async def _send_request(
        self,
        command: str,
        arguments: DapBaseModel | None = None,
        timeout: float = 5.0,
    ) -> dict[str, Any]:
        ...


class ZxdbDapMixin:
    """Mixin for zxdb-specific DAP extensions."""

    async def zxdb_detach(
        self: SupportsSendRequest,
        args: ZxdbDetachArguments,
    ) -> dict[str, Any]:
        """Sends a custom zxdb detach request."""
        return await self._send_request("zxdb.Detach", args)

    async def zxdb_process(
        self: SupportsSendRequest,
        args: ZxdbProcessArguments | None = None,
    ) -> ZxdbProcessResponse:
        """Sends a custom zxdb process query request."""
        resp = await self._send_request(
            "zxdb.Process", args or ZxdbProcessArguments()
        )
        return ZxdbProcessResponse.model_validate(resp)

    async def zxdb_stack_trace(
        self: SupportsSendRequest,
        args: ZxdbStackTraceArguments,
    ) -> StackTraceResponse:
        """Sends a custom zxdb stackTrace request."""
        resp = await self._send_request("stackTrace", args)
        return StackTraceResponse.model_validate(resp)

    async def zxdb_threads(
        self: SupportsSendRequest,
    ) -> ZxdbThreadsResponse:
        """Sends a threads request.

        Returns:
            ZxdbThreadsResponse: Response containing zxdb threads.
        """
        resp = await self._send_request("threads")
        return ZxdbThreadsResponse.model_validate(resp)

    async def zxdb_pause_process(
        self: SupportsSendRequest,
        args: ZxdbPauseArguments,
    ) -> Response:
        """Sends a pause request with process scope."""
        resp = await self._send_request("pause", args)
        return Response.model_validate(resp)
