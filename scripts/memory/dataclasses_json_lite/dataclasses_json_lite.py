# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Deserialize python dataclass from json.

This is a minimal drop-in replacement from dataclasses_json.

Dataclasses annotated with `@dataclass_json` gets a `from_dict` factory class method.

Members can be renamed with `field` metadata:

    from dataclasses import dataclass, field

    @dataclass_json
    @dataclass
    class Process:
        koid: int
        name: str
        vmos_koid: List[int] = field(metadata=config(field_name="vmos"))

The configuration also makes possible to override the conversion from json to the field.
"""
import dataclasses
import sys
import types
from collections import abc
from enum import Enum
from typing import (
    Any,
    Callable,
    Dict,
    ForwardRef,
    Optional,
    Union,
    get_args,
    get_origin,
)

_PYTHON_3_12 = sys.version_info >= (
    3,
    12,
)


class Undefined(Enum):
    """Missing fields always fails. Added for API compatibility."""

    RAISE = 1


def config(
    field_name: Optional[str] = None,
    decoder: Optional[Callable[[Any], Any]] = None,
) -> Dict[str, Any]:
    """Returns a field metadata dictionary that override the default field name and deserialization.

    Args:
        field_name: JSON object field name holding the data for this field. The dataclass field name
            is used by default.
        decoder: function receiving the JSON value and returning a value to be assigned to the
            dataclass field. `_default_decoder` is used by default.

    Returns:
    """
    return dict(field_name=field_name, decoder=decoder)


def _identity(x: Any) -> Any:
    return x


def _default_decoder(cls: type, field_type: type) -> Callable[[Any], Any]:
    origin = get_origin(field_type)
    if origin is list or origin is abc.Sequence:
        inner_types = get_args(field_type)
        assert len(inner_types) == 1, f"Supports only list with one type"
        inner_decoder = _default_decoder(cls, inner_types[0])
        return lambda json_data: [inner_decoder(e) for e in json_data]
    elif origin is Union or origin is types.UnionType:
        inner_type, likely_null = get_args(field_type)
        assert likely_null is type(None), "Supports only Optional"
        return _default_decoder(cls, inner_type)
    elif dataclasses.is_dataclass(field_type):
        assert hasattr(field_type, "from_dict"), (
            f"Class {field_type} does not have 'from_dict'. "
            "Did you annotated with `@dataclass_json()`"
        )
        return field_type.from_dict
    elif isinstance(field_type, ForwardRef):
        resolved_decoder = None

        def decode_forward_ref(x):
            nonlocal resolved_decoder
            if resolved_decoder:
                return resolved_decoder(x)
            evaluate_kwargs = dict()
            if _PYTHON_3_12:
                evaluate_kwargs["recursive_guard"] = frozenset()
            resolved_decoder = field_type._evaluate(
                sys.modules.get(cls.__module__).__dict__,
                vars(cls),
                set(),
                **evaluate_kwargs,
            ).from_dict
            return resolved_decoder(x)

        return decode_forward_ref
    else:
        return _identity


def dataclass_json(
    undefined: Optional[Undefined] = None,
) -> Callable[[type], type]:
    """Annotates a dataclass, adding a `from_dict` method that deserialises a json value."""
    del undefined

    def decorator(cls: type) -> type:
        assert dataclasses.is_dataclass(cls)
        json_to_py = dict()
        for field in dataclasses.fields(cls):
            json_key = field.metadata.get("field_name", field.name)
            decoder = field.metadata.get("decoder") or _default_decoder(
                cls, field.type
            )
            json_to_py[json_key or field.name] = (field.name, decoder)

        def from_dict(data_dict: Dict[str, Any]) -> Any:
            kws = dict()
            for key, value in data_dict.items():
                try:
                    field_name, decoder = json_to_py[key]
                    kws[field_name] = decoder(value)
                except Exception as e:
                    raise ValueError(
                        f"Unable to decode field {key!r} of {cls.__name__!r}"
                    ) from e
            return cls(**kws)

        assert not hasattr(cls, "from_dict")
        cls.from_dict = from_dict  # type: ignore[attr-defined]
        return cls

    return decorator
