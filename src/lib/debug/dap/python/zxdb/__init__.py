# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from pydap.client import DapClient
from pydap.models import ThreadEvent
from zxdb_dap.models import (
    AsyncBacktraceUpdate,
    AsyncTaskNode,
    ZxdbPauseArguments,
    ZxdbProcessStoppedEvent,
    ZxdbProcessStoppedEventBody,
    ZxdbThread,
    ZxdbThreadEvent,
    ZxdbThreadEventBody,
    ZxdbThreadsResponse,
    ZxdbThreadsResponseBody,
)
from zxdb_dap.zxdb_dap_mixin import (
    ZxdbDapMixin,
    ZxdbDetachArguments,
    ZxdbProcessArguments,
    ZxdbProcessInfo,
    ZxdbProcessResponse,
    ZxdbProcessResponseBody,
    ZxdbStackTraceArguments,
)


class ZxdbDapClient(ZxdbDapMixin, DapClient):
    pass


__all__ = [
    # keep-sorted start
    "AsyncBacktraceUpdate",
    "AsyncTaskNode",
    "ThreadEvent",
    "ZxdbDapClient",
    "ZxdbDapMixin",
    "ZxdbDetachArguments",
    "ZxdbPauseArguments",
    "ZxdbProcessArguments",
    "ZxdbProcessInfo",
    "ZxdbProcessResponse",
    "ZxdbProcessResponseBody",
    "ZxdbProcessStoppedEvent",
    "ZxdbProcessStoppedEventBody",
    "ZxdbStackTraceArguments",
    "ZxdbThread",
    "ZxdbThreadEvent",
    "ZxdbThreadEventBody",
    "ZxdbThreadsResponse",
    "ZxdbThreadsResponseBody",
    # keep-sorted end
]
