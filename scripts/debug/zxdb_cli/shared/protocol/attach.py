# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "attach"


class AttachRequest(BaseRequest[dict[str, Any]]):
    """Request to attach to a process."""

    command: Literal["attach"] = COMMAND_NAME
    # Place 'int' first in the Union to avoid Pydantic standard coercion of
    # PIDs to strings.
    filter: int | str
    response_type: ClassVar[Any] = dict[str, Any]
