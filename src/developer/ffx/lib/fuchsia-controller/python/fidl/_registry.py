# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import typing

_METHOD_REGISTRY: dict[int, tuple[typing.Any | None, typing.Any | None]] = {}


def register_method(
    ordinal: int,
    request_cls: typing.Any | None,
    response_cls: typing.Any | None,
) -> None:
    _METHOD_REGISTRY[ordinal] = (request_cls, response_cls)


def get_registered_method(
    ordinal: int,
) -> tuple[typing.Any | None, typing.Any | None] | None:
    return _METHOD_REGISTRY.get(ordinal)


def get_method_ordinal(class_name: str) -> int | None:
    for ord_val, (req_cls, resp_cls) in _METHOD_REGISTRY.items():
        if (req_cls and req_cls.__name__ == class_name) or (
            resp_cls and resp_cls.__name__ == class_name
        ):
            return ord_val
    return None
