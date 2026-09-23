# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import re
import sys
import typing
from abc import ABCMeta
from collections.abc import Sequence
from dataclasses import dataclass
from enum import IntEnum
from typing import Optional

import fuchsia_controller_py as fc

# These can be updated to use TypeAlias when python is updated to 3.10+
TXID_Type = int
Ordinal = int

FidlMessage = tuple[
    bytes, Sequence[fc.BaseHandle] | Sequence[tuple[int, int, int, int, int]]
]

# The number of bytes in a FIDL header.
FIDL_HEADER_SIZE = 8
# The number of bytes in a FIDL ordinal.
FIDL_ORDINAL_SIZE = 8
FIDL_EPITAPH_ORDINAL = 0xFFFFFFFFFFFFFFFF


class FidlMeta(ABCMeta):
    def __new__(
        meta_cls,
        name: str,
        bases: tuple[type, ...],
        classdict: dict[str, typing.Any],
        required_class_variables: list[tuple[str, type]] | None = None,
    ) -> "FidlMeta":
        if required_class_variables is None:
            required_class_variables = []

        def _check_required_class_variables(cls: type["FidlMeta"]) -> None:
            # These class variables are always required.
            if required_class_variables is not None:
                required_class_variables.extend([("__fidl_kind__", str)])

                missing_class_variables = []
                for var, ty in required_class_variables:
                    if not hasattr(cls, var) or not isinstance(
                        getattr(cls, var), ty
                    ):
                        missing_class_variables.append(
                            (var, type(getattr(cls, var, None)))
                        )
                if len(missing_class_variables) > 0:
                    raise NotImplementedError(
                        f"Class {cls} missing (or wrong type) required\nclass variables:\n  {missing_class_variables}"
                    )

        # If it exists, run the class-defined __init_subclass__ after the injected one.
        if "__init_subclass__" in classdict:
            class_defined_init_subclass = classdict["__init_subclass__"]

            def wrapper(cls: type["FidlMeta"]) -> None:
                _check_required_class_variables(cls)
                class_defined_init_subclass(cls)

            classdict["__init_subclass__"] = wrapper
        else:
            classdict["__init_subclass__"] = _check_required_class_variables

        return super().__new__(meta_cls, name, bases, classdict)


class FidlProtocolMarker(
    str,
    metaclass=FidlMeta,
):
    ...


@dataclass
class MethodInfo:
    name: str
    request_ident: str
    requires_response: bool
    empty_response: bool
    has_result: bool
    response_identifier: str | None


@dataclass
class DomainError:
    """A class used to wrap returning an error from a two-way method."""

    error: typing.Any


class StopEventHandler(Exception):
    """An exception used to stop an event handler from continuing."""


class FrameworkError(IntEnum):
    UNKNOWN_METHOD = -2


def internal_kind_to_type(internal_kind: str) -> type[FrameworkError]:
    # TODO(https://fxbug.dev/42061151): Remove "transport_error".
    if internal_kind == "framework_error" or internal_kind == "transport_error":
        return FrameworkError
    raise RuntimeError(f"Unrecognized internal type: {internal_kind}")


class EpitaphError(fc.ZxStatus):
    """An exception received when an epitaph has been sent on the channel."""


@dataclass
class GenericResult:
    fidl_type: str
    response: Optional[object] = None
    err: Optional[object] = None
    framework_err: FrameworkError | None = None

    @property
    def __fidl_type__(self) -> str:
        return self.fidl_type

    @property
    def __fidl_raw_type__(self) -> str:
        return self.fidl_type


def parse_txid(msg: FidlMessage) -> int:
    (b, _) = msg
    return int.from_bytes(b[0:4], sys.byteorder)


def parse_ordinal(msg: FidlMessage) -> int:
    (b, _) = msg
    start = FIDL_HEADER_SIZE
    end = FIDL_HEADER_SIZE + FIDL_ORDINAL_SIZE
    return int.from_bytes(b[start:end], sys.byteorder)


def parse_epitaph_value(msg: FidlMessage) -> int:
    (b, _) = msg
    start = FIDL_HEADER_SIZE + FIDL_ORDINAL_SIZE
    end = start + 4
    res = int.from_bytes(b[start:end], sys.byteorder)

    # Do two's complement here to get the true value if it's negative.
    if res & (1 << 31) != 0:
        return res - (1 << 32)
    return res


def camel_case_to_snake_case(s: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", s).lower()


def normalize_identifier(identifier: str) -> str:
    """Takes an identifier and attempts to normalize it.

    For the average identifier this shouldn't do anything. This only applies to result types
    that have underscores in their names.

    Returns: The normalized identifier string (sans-underscores).
    """
    if identifier.endswith("_Result") or identifier.endswith("_Response"):
        return identifier.replace("_", "")
    return identifier


class Unsupported:
    """Sentinel type used by fidlgen_python to stub out unsupported FIDL constructs.

    When fidlgen_python encounters unsupported types (such as structs with deprecated
    defaults), it generates stub classes whose `__init__` requires an instance of
    `fidl.Unsupported`. Because `Unsupported` raises `NotImplementedError` when
    instantiated, this prevents construction of unsupported FIDL types at runtime.
    """

    def __init__(self, _unsupported: typing.Self) -> None:
        raise NotImplementedError


class Encodable(typing.Protocol):
    def encode(self) -> tuple[bytes, list[typing.Any]]:
        ...


class Decodable(typing.Protocol):
    @classmethod
    def decode(cls, data: bytes, handles: list[int]) -> typing.Self:
        ...
