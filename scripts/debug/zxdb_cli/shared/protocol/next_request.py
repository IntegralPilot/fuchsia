# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from pydap.dap_types import SteppingGranularity
from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "next"


class NextRequest(BaseRequest[dict[str, Any]]):
    """Request to step over to the next line of code."""

    command: Literal["next"] = COMMAND_NAME
    thread_id: int
    single_thread: bool | None = None
    granularity: SteppingGranularity | None = None
    response_type: ClassVar[Any] = dict[str, Any]
