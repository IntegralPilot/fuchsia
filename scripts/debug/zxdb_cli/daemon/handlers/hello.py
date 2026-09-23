# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from shared.protocol.base import PROTOCOL_VERSION, Response
from shared.protocol.hello import (
    COMMAND_NAME,
    HelloRequest,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, req: HelloRequest) -> Response[Any]:
    """Handles the hello handshake request.

    Verifies the protocol version.
    """
    if req.version != PROTOCOL_VERSION:
        return Response(
            success=False,
            message=(
                f"Protocol version mismatch. CLI version: {req.version}, "
                f"Daemon version: {PROTOCOL_VERSION}"
            ),
        )

    return Response(success=True, body={"protocol_version": PROTOCOL_VERSION})
