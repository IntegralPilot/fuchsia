# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "break"


class BreakRequest(BaseRequest[dict[str, Any]]):
    """Request to set or delete a breakpoint at a file and line.

    The file path must be fully qualified from the workspace root, an absolute
    path, or exist relative to the current working directory.
    To delete a breakpoint, set delete=True and ensure the file and line match
    an existing breakpoint.
    To view currently installed breakpoints, use the get-state command.
    """

    command: Literal["break"] = COMMAND_NAME
    file: str
    line: int
    delete: bool = False
    response_type: ClassVar[Any] = dict[str, Any]
