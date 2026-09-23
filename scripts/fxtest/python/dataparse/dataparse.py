# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Decorator for dataclass interoperability with dicts.

Typical Usage:
from dataparse import dataparse
from dataclasses import dataclass

@dataparse
@dataclass
class WeatherInfo:
    temperature_celsius: float
    rain_percent: float
    weather_description: str

weather = WeatherInfo.from_dict({
  'temperature_celsius': 12.3,
  'rain_percent': 23.3,
  'weather_description': 'Scattered Showers',
  'barometric_pressure': 29.9,
})

print(weather.to_dict())
"""

import dataclasses
import enum
import types
import typing


class DataParseError(Exception):
    """Raised when there was an issue matching data against a schema defined by a dataclass."""


T = typing.TypeVar("T")


def dataparse(cls: T) -> T:
    """Decorator for dataclass to support converting to and from dictionaries.

    dataclasses.dataclass allows for typed fields to be set by an
    incoming dictionary. Unfortunately, passing a field that is not
    known to the dataclass in the input results in an ArgumentError.
    The @dataparse decorator adds `to_dict` and `from_dict` methods
    (if not already set) to the class to support partial matching
    of input. It additionally provides basic type safety checks,
    and supports recursive definitions.

    Only the following types of fields are supported:
    - int, float, str.
    - Classes decorated with @dataparse
    - Lists and sets of the above.
    - Enums with str values (note: lists of enums are not supported yet)

    Generated Methods:
        dataparse_renames (classmethod): Returns a mapping from
            local variable name to serialized name. For example: {'foo':
            'Foo Value'} will use dictionary key 'Foo Value' to populate
            the field 'foo'.
        to_dict: Returns the serialized value of the object. If set
            on the class, this decorator will not override.
        from_dict (classmethod): Constructs an instance of the class
            based on an incoming dictionary. If set on the class, this
            decorator will not override.

    Args:
        cls: A class to be wrapped.

    Returns:
        A class with additional methods added for interoperability with dicts.

    Raises:
        DataParseError: It is not valid to wrap the given class,
        likely because it was not already @dataclass.
    """
    if not dataclasses.is_dataclass(cls):
        raise DataParseError(
            "@dataparse may only be used on a @dataclass. Make sure your class is wrapped with dataclasses.dataclass first."
        )

    class_fields = dataclasses.fields(cls)

    if not hasattr(cls, "dataparse_renames"):
        setattr(cls, "dataparse_renames", classmethod(lambda self: dict()))
    renames: dict[str, str] = getattr(cls, "dataparse_renames")()

    def to_dict(self: T) -> dict[str, typing.Any]:
        out_dict = {}
        f: dataclasses.Field  # type: ignore
        for f in class_fields:
            name = renames.get(f.name, f.name)

            val = getattr(self, f.name)
            if hasattr(val, "to_dict"):
                # Recursively do to_dict if available, otherwise
                # just use the incoming value
                val = val.to_dict()
            if isinstance(val, list) or isinstance(val, set):
                if isinstance(val, set):
                    # Ensure that both lists and sets are stored
                    # as lists, so they can be converted to JSON.
                    try:
                        # Attempt to sort set values for easier comparison.
                        origin_vals = sorted(list(val))
                    except TypeError:
                        origin_vals = list(val)
                else:
                    origin_vals = val

                val = [
                    x.to_dict() if hasattr(x, "to_dict") else x
                    for x in origin_vals
                ]
            if isinstance(val, enum.Enum):
                val = val.value

            if val is not None:
                # Omit null fields.
                out_dict[name] = val

        return out_dict

    if not hasattr(cls, "to_dict"):
        setattr(cls, "to_dict", to_dict)

    def from_dict(_: typing.Any, input: dict[str, typing.Any]) -> T:
        build_args: dict[str, typing.Any] = dict()

        f: dataclasses.Field  # type: ignore
        for f in class_fields:

            def load_real_type(
                incoming_type: typing.Any,
            ) -> tuple[typing.Any, typing.Any]:
                """Recursively determine the real type of an incoming type.

                Some field types refer to typing-module aliases
                rather than the real type at hand, specifically
                list (list[T]), set (typing.Set[T]), and
                optional types (typing.Optional[T]).

                This method determines two type values: the actual
                type of the field as well as the argument type in
                the case of collections.

                Examples:
                >>> load_real_type(list[int])
                (list, int)
                >>> load_real_type(typing.Set[float])
                (set, float)
                >>> load_real_type(typing.Optional[int])
                (int, None)
                >>> load_real_type(int)
                (int, None)

                Args:
                    incoming_type: The type to process.

                Raises:
                    DataParseError: If dataparse doesn't support
                        this type. For example, Union[int, float], or
                        any Union whose second argument is not None.

                Returns:
                    Tuple of (base type, optional argument type).
                """

                def load_union_type() -> typing.Any:
                    # Load the union type from incoming_type.
                    # Only support Optional unions.
                    args = incoming_type.__args__
                    if len(args) != 2 or type(None) not in args:
                        raise DataParseError(
                            "Invalid Union type for dataparse. We support only Optional unions with a single type: "
                            + str(args)
                        )
                    inner_type = args[0] if args[1] == type(None) else args[1]
                    return load_real_type(inner_type)

                if hasattr(incoming_type, "__origin__"):
                    origin = incoming_type.__origin__
                    if origin == list:
                        return (list, incoming_type.__args__[0])
                    elif origin == set:
                        return (set, incoming_type.__args__[0])
                    elif origin == typing.Union:
                        return load_union_type()
                if isinstance(incoming_type, types.UnionType):
                    return load_union_type()

                return (incoming_type, None)

            real_type, real_args = load_real_type(f.type)

            name = renames.get(f.name, f.name)
            if name in input:
                if input[name] is None:
                    # Copy incoming nulls verbatim.
                    build_args[f.name] = None
                elif hasattr(real_type, "from_dict"):
                    # Handle recursive from_dict case.
                    build_args[f.name] = real_type.from_dict(input[name])
                elif real_type == list:
                    # Handle parsing lists.
                    build_args[f.name] = [
                        (
                            real_args.from_dict(val)
                            if real_args is not None
                            and hasattr(real_args, "from_dict")
                            else val
                        )
                        for val in input[name]
                    ]
                elif real_type == set:
                    # Handle parsing sets.
                    # We store sets as a list of the elements. Convert back to set on read.
                    build_args[f.name] = set(
                        [
                            (
                                real_args.from_dict(val)
                                if real_args is not None
                                and hasattr(real_args, "from_dict")
                                else val
                            )
                            for val in input[name]
                        ]
                    )
                elif isinstance(real_type, type) and issubclass(
                    real_type, enum.Enum
                ):
                    # Handle enums.
                    # The value stored in the incoming dict is the value of
                    # the enum. Create a map of value name to enum, and
                    # select the correct one if it is present.
                    v: enum.Enum
                    values = {
                        v.value: v for v in real_type.__members__.values()
                    }
                    if input[name] not in values:
                        raise DataParseError(
                            f"Enum {real_type} is missing field {input[name]}"
                        )
                    build_args[f.name] = values[input[name]]
                else:
                    # Handle all other types by assigning directly.
                    build_args[f.name] = input[name]

        try:
            return cls(**build_args)  # type: ignore
        except TypeError as e:
            raise DataParseError(
                f"Failed to instantiate {getattr(cls, '__name__', str(cls))} from dict: {e}"
            ) from e

    if not hasattr(cls, "from_dict"):
        setattr(cls, "from_dict", classmethod(from_dict))

    return cls  # type: ignore
