# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "start"


class StartRequest(BaseRequest[dict[str, Any]]):
    """Request to start the debugging session."""

    command: Literal["start"] = COMMAND_NAME
    port: int | None = None
    connect: bool = False
    response_type: ClassVar[Any] = dict[str, Any]
