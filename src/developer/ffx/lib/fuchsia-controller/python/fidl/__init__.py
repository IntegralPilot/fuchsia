# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import typing

from ._async_channel import AsyncChannel
from ._async_socket import AlreadyReadingAll, AsyncSocket
from ._decoder import Decoder, decode_struct
from ._encoder import Encoder, encode_struct
from ._fidl_common import (
    DomainError,
    EpitaphError,
    FrameworkError,
    StopEventHandler,
    Unsupported,
)
from ._ipc import GlobalHandleWaker, HandleWaker
from ._registry import (
    get_method_ordinal,
    get_registered_method,
    register_method,
)

__all__ = [
    "AlreadyReadingAll",
    "AsyncSocket",
    "AsyncChannel",
    "DomainError",
    "EpitaphError",
    "FrameworkError",
    "GlobalHandleWaker",
    "HandleWaker",
    "StopEventHandler",
    "Unsupported",
    "Encoder",
    "encode_struct",
    "Decoder",
    "decode_struct",
    "register_method",
    "get_registered_method",
    "get_method_ordinal",
]
