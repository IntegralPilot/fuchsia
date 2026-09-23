# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "stop"


class StopRequest(BaseRequest[None]):
    """Request to stop the daemon and session."""

    command: Literal["stop"] = COMMAND_NAME
    response_type: ClassVar[Any] = None
