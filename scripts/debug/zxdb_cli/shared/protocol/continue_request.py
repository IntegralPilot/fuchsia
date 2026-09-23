# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "continue"


class ContinueRequest(BaseRequest[dict[str, Any]]):
    """Request to resume execution of a thread."""

    command: Literal["continue"] = COMMAND_NAME
    thread_id: int
    single_thread: bool | None = None
    response_type: ClassVar[Any] = dict[str, Any]
