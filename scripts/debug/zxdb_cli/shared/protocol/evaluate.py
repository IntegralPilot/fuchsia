# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from pydantic import BaseModel, ConfigDict
from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "evaluate"


class EvaluateResponse(BaseModel):
    """Unified response containing the evaluation result and optional children."""

    model_config = ConfigDict(extra="forbid")

    result: str | None = None
    type: str | None = None


class EvaluateRequest(BaseRequest[EvaluateResponse]):
    """Request evaluation of an expression in a given stack frame."""

    command: Literal["evaluate"] = COMMAND_NAME
    thread_id: int

    # Note that this is the index of the frame in this particular thread's stack. It is _not_ the
    # frameId value communicated from the DAP server. This depends on the DAP server sending
    # monotonically increasing frameIds that are non-overlapping and unique across threads.
    frame_index: int = 0
    expression: str
    response_type: ClassVar[Any] = EvaluateResponse
