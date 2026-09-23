# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Any, ClassVar, Final, Literal

from shared.protocol.base import BaseRequest

COMMAND_NAME: Final = "threads"


class ThreadsRequest(BaseRequest[dict[str, Any]]):
    """Request list of threads."""

    command: Literal["threads"] = COMMAND_NAME
    response_type: ClassVar[Any] = dict[str, Any]
