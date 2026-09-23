# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any

from shared.protocol.base import Response
from shared.protocol.wait_for_event import (
    COMMAND_NAME,
    WaitForEventRequest,
)

__all__ = ["COMMAND_NAME", "handle"]

if TYPE_CHECKING:
    from daemon.daemon import Daemon


async def handle(daemon: Daemon, req: WaitForEventRequest) -> Response[Any]:
    """Blocks until there are events with sequence number greater than
    last_seen_seq.

    Args:
        daemon: The Daemon instance.
        req: The request containing last_seen_seq.

    Returns:
        A Response containing the new events.
    """
    timeout = req.timeout

    async with daemon.new_event_condition:
        try:
            # Wait until there is an event with seq > last_seen_seq.
            # We check the last event's sequence number.
            while daemon.latest_seq <= req.last_seen_seq:
                if timeout is not None:
                    await asyncio.wait_for(
                        daemon.new_event_condition.wait(),
                        timeout=timeout,
                    )
                else:
                    await daemon.new_event_condition.wait()
        except asyncio.TimeoutError:
            return Response(
                success=False, message="Timed out waiting for event"
            )

    events = []
    for seq in range(req.last_seen_seq + 1, daemon.latest_seq + 1):
        if seq in daemon.all_events:
            events.append(daemon.all_events[seq])

    message = None
    if (
        daemon.all_events
        and daemon.all_events[next(iter(daemon.all_events))].get("seq", 0)
        > req.last_seen_seq + 1
    ):
        message = "Warning: Some events were pruned from history"

    return Response(success=True, events=events, message=message)
