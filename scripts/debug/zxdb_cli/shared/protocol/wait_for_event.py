# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "wait-for-event"


class WaitForEventRequest(BaseRequest[None]):
    """Request to wait for a debug adapter event."""

    command: Literal["wait-for-event"] = COMMAND_NAME
    last_seen_seq: int  # Overridden to be required
    timeout: int | None = None
    response_type: ClassVar[Any] = None
