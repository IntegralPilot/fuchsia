# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Literal

from pydantic import Field
from pydap.dap_types import DapBaseModel, Thread
from pydap.models import (
    Event,
    PauseArguments,
    ThreadEvent,
    ThreadEventBody,
    ThreadsResponse,
    ThreadsResponseBody,
)


class ZxdbPauseArguments(PauseArguments):
    """Arguments for `pause` request with zxdb extensions.

    Attributes:
        process_id: Optional process ID to pause.
    """

    # We have to explicitly override thread_id: int = Field(default=0) on
    # ZxdbPauseArguments because Pydantic v2 inherits field requiredness
    # from the parent class when not overridden.
    thread_id: int = Field(default=0)
    process_id: int


class ZxdbProcessStoppedEventBody(DapBaseModel):
    """Body of the `processStopped` event.

    Attributes:
        process_id: Process ID (KOID).
        name: Process name.
        threads: List of thread IDs (KOIDs) stopped in the process.
    """

    process_id: int
    name: str | None = None
    threads: list[int] = Field(default_factory=list)


class ZxdbProcessStoppedEvent(Event):
    """Zxdb-specific processStopped event."""

    type: Literal["event"] = "event"
    event: Literal["processStopped"] = "processStopped"
    body: ZxdbProcessStoppedEventBody


class ZxdbThread(Thread):
    """Zxdb-specific thread model containing processId."""

    process_id: int | None = None


class ZxdbThreadsResponseBody(ThreadsResponseBody):
    """Body of response to `threads` request containing zxdb threads."""

    threads: list[ZxdbThread]


class ZxdbThreadsResponse(ThreadsResponse):
    """Response to `threads` request with zxdb specific thread info."""

    body: ZxdbThreadsResponseBody


class ZxdbThreadEventBody(ThreadEventBody):
    """Body of the thread event containing optional processId."""

    process_id: int | None = None


class ZxdbThreadEvent(ThreadEvent):
    """Zxdb-specific thread event with processId in body."""

    body: ZxdbThreadEventBody


class AsyncTaskNode(DapBaseModel):
    """Represents a node in the async task tree."""

    id: str | None = None
    name: str | None = None
    file: str | None = None
    line: int | None = None
    children: list["AsyncTaskNode"] = Field(default_factory=list)


class AsyncBacktraceUpdateBody(DapBaseModel):
    """Body of zxdb.updateAsyncBacktrace event.

    Attributes:
        thread_id: Thread KOID (aliased as 'id' in DAP wire protocol).
        name: Name of the thread.
        process_id: Process KOID.
        tasks: Root list of async task nodes.
    """

    thread_id: int = Field(alias="id")
    name: str | None = None
    process_id: int
    tasks: list[AsyncTaskNode] = Field(default_factory=list)


class AsyncBacktraceUpdate(Event):
    """Custom event for asynchronous backtrace updates."""

    type: Literal["event"] = "event"
    event: Literal["zxdb.updateAsyncBacktrace"] = "zxdb.updateAsyncBacktrace"
    body: AsyncBacktraceUpdateBody


AsyncTaskNode.model_rebuild()
