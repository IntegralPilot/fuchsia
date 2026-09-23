# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from pydantic import model_validator
from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "detach"


class DetachRequest(BaseRequest[dict[str, Any]]):
    """Request to detach from a process."""

    command: Literal["detach"] = COMMAND_NAME
    pid: int | None = None
    all: bool = False
    response_type: ClassVar[Any] = dict[str, Any]

    @model_validator(mode="after")
    def validate(self) -> "DetachRequest":
        if self.all and self.pid is not None:
            raise ValueError("Cannot specify both PID and all")
        if not self.all and self.pid is None:
            raise ValueError("PID is required when all is not specified")
        return self
