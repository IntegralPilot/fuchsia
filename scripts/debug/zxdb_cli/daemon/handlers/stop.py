# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from shared.protocol.base import Response
from shared.protocol.stop import (
    COMMAND_NAME,
    StopRequest,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, _req: StopRequest) -> Response[Any]:
    daemon.stop_event.set()
    return Response(success=True, message="Daemon stopping")
