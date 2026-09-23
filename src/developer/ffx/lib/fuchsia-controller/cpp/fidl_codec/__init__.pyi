# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This stub file is used for both mypy typechecking at compile time, as well as IDE assistance
# via pyright/pylance.

# Note: this file doesn't include everything, just all the functions actually in use.
from typing import Any, TypeAlias

_handles: TypeAlias = list[int] | list[tuple[int, int, int, int, int]]

def decode_fidl_response(bytes: bytes, handles: _handles) -> dict[str, Any]: ...
def decode_fidl_request(bytes: bytes, handles: _handles) -> dict[str, Any]: ...

# It may be better to specify that this is a `dict[str, Any]`, but there are several
# conformance tests that rely on getting an arbitrary attr form this type.
# In addition, developers shouldn't be using this directly anyway.
def decode_standalone(
    type_name: str, bytes: bytes, handles: _handles
) -> Any: ...
def encode_fidl_message(
    object: object | None,
    library: str | None,
    type_name: str | None,
    txid: int,
    ordinal: int,
) -> tuple[bytes, list[tuple[int, int, int, int, int]]]: ...
def encode_fidl_object(
    object: object | None, library: str | None, type_name: str | None
) -> tuple[bytes, list[tuple[int, int, int, int, int]]]: ...
def add_ir_path(path: str) -> None: ...
def method_ordinal(protocol: str, method: str) -> int: ...
