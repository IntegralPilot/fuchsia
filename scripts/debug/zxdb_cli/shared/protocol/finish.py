# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "finish"


class FinishRequest(BaseRequest[dict[str, Any]]):
    """Request to finish execution of current frame (step out)."""

    command: Literal["finish"] = COMMAND_NAME
    thread_id: int
    single_thread: bool | None = None
    response_type: ClassVar[Any] = dict[str, Any]
