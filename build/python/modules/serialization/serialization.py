# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Serialization Methods for Classes.

This module is inspired by the `serde` crate in Rust, and the dataclasses python
module.

It provides a type-aware mechanism for serializing and deserializing classes
to/from dictionaries of string key:values, and then from those to/from JSON.

The module relies on PEP-526 type annotations in order to function.  If members
of the class are missing type annotations, it will not function correctly.

Example:

    from dataclasses import dataclass, field
    from typing import Dict, List, Optional
    from serialization import instance_to_dict, json_dumps, json_loads

    @dataclass
    class Child:
        name: str
        height: Optional[int] = None
        interests: list[str] = field(default_factory=list)

    @dataclass
    class Parent:
        name: str
        children: list[Child] = field(default_factory=list)

    parent = Parent("Some Person")
    parent.children.append(Child("A Child", 130, ["reading", "cats"]))

    print(parent)
    print("\n")
    print(instance_to_dict(parent))

will result in:

    Parent(name='Some Person', children=[Child(name='A Child', height=130,
    interests=['reading', 'cats'])])

    {'name': 'Some Person', 'children': [{'name': 'A Child', 'height': 130,
    'interests': ['reading', 'cats']}]}

and:

    print(json_dumps(parent, indent=2))

will yield:

    {
      "name": "Some Person",
      "children": [
        {
          "name": "A Child",
          "height": 130,
          "interests": [
            "reading",
            "cats"
          ]
        }
      ]
    }

Parsing is similarly straightforward:

    json_string = '{"name":"First Last","children":[\
        {"name":"a child", "interests":["toys", "games"]}]}
    parent = json_loads(Parent, json_string)
    print(parent)

yields:

    Parent(name='First Last', children=[Child(name='a child', height=None,
    interests=['toys', 'games'])])

Which can then be used as the object that it is:

    for line in ['{} likes {}'.format(child.name, " and ".join(child.interests))
                for child in parent.children]:
        print(line)

to print:

    a child likes toys and games

"""

import dataclasses
import functools
import inspect
import io
import json
import types
import typing
from collections.abc import Callable
from typing import Any, TypeVar, Union, get_type_hints

__all__ = [
    "instance_from_dict",
    "instance_to_dict",
    "json_dump",
    "json_dumps",
    "json_load",
    "json_loads",
    "serialize_dict",
    "serialize_json",
    "serialize_fields_as",
    "JSONValue",
]


class JSONValue:
    """Marker class representing an arbitrary JSON value.

    This class has no runtime functionality and will raise an exception if instantiated.
    It serves solely as a type hint to indicate that a field contains raw,
    unvalidated JSON data (e.g., a dict, list, or primitive).

    Consumers using static type checking (like Mypy) should use `isinstance`
    checks or `typing.cast` to narrow the type when accessing the value.
    """

    def __init__(self) -> None:
        raise RuntimeError("JSONValue cannot be instantiated.")


# The placeholder for the Class of the object that's being serialized or
# deserialized.
C = TypeVar("C")

####
#
#  Deserialization Flow
#


def instance_from_dict(cls: type[C], a_dict: dict[str, Any]) -> C:
    """Instantiate an object from a dictionary of its field values.

    The default strategy is to instantiate a default instance of the class, and
    then set any fields whose values are found in the dictionary.

    This relies on type annotations that specify the class of each field and
    each init param.  All must match names in the entry dictionary.
    """
    init_param_types = _get_fn_param_types(cls, "__init__")
    init_param_values = _get_named_values_from(a_dict, init_param_types, cls)
    instance = cls(**init_param_values)

    field_types = typing.get_type_hints(cls)

    # Strip the fields that were provided via the constructor.
    fields_to_read = {
        name: field_type
        for name, field_type in field_types.items()
        if name not in init_param_types
    }

    # Now, validate that the dict doesn't contain anything extraneous by taking
    # the set of keys in the supplied dictionary, and removing from it the names
    # of all the fields, and if any keys are left, then error out.
    supplied_names = set(a_dict.keys()) - set(field_types.keys())
    if len(supplied_names) != 0:
        raise KeyError(
            f"Unknown fields found when deserializing {cls.__name__}: {', '.join(supplied_names)}"
        )

    # If any fields weren't set via the constructor, set those attributes on the
    # class directly.  For classes that use @dataclass, this is likely to be
    # empty.
    named_values = _get_named_values_from(a_dict, fields_to_read, cls)
    for field_name, value in named_values.items():
        setattr(instance, field_name, value)

    return instance


def _get_fn_param_types(cls: type[Any], fn_name: str) -> dict[str, type[Any]]:
    """Get the names and types of the parameters of the fn with the given name
    for the given class.

    Strips out 'self', and only returns the other params.
    """
    fn_sig = inspect.signature(cls.__dict__[fn_name])
    return {
        name: parameter.annotation
        for name, parameter in fn_sig.parameters.items()
        if name != "self"
    }


def _get_named_values_from(
    entries: dict[str, Any],
    names_and_types: dict[str, type[Any]],
    for_cls: type[Any],
) -> dict[str, Any]:
    """Take a Dict of name:value, and a dict of name:types, and return a dict of
    name to instantiated-type-for-value, for the class given by `for_cls`.

    The given class is used to determine if default values have been provided or
    not.

    Example::
        >>> entry = { "my_int": "42", "some_vals": [ "1", "2", "3" ]}
        >>> names_and_types = { "my_int":"int", "some_vals": "list[int]", "has_a_default": "int" }
        >>> get_named_values_from(entry, names_and_types, my_class)
        {"my_int":42, "some_vals":[1, 2, 3]}

    """
    values: dict[str, Any] = {}
    for name, cls in names_and_types.items():
        if name in entries:
            values[name] = _parse_value_into(entries[name], cls)
        else:
            has_default = _has_default_value(for_cls, name)
            if not has_default:
                raise KeyError(
                    "unable to find required value for '{}' in: {}".format(
                        name, entries.keys()
                    )
                )

    return values


def _has_default_value(cls: type[Any], field: str) -> Any:
    """Returns if the given field of a class has a default value (requires a
    @dataclass-based class, others will silently return 'False')
    """
    dataclass_fields: dict[str, Any] | None = getattr(
        cls, "__dataclass_fields__", None
    )
    if dataclass_fields:
        field_entry = dataclass_fields.get(field, None)
        if field_entry:
            has_value = field_entry.default is not dataclasses.MISSING
            has_factory = field_entry.default_factory is not dataclasses.MISSING
            return has_value or has_factory
    return False


def _parse_value_into(
    value: Any,
    cls: type[Any],
) -> dict[str, Any] | list[Any] | set[Any] | C | None:
    """For a class, attempt to parse it from the value."""
    if value is None:
        return None

    if cls is Any:
        raise TypeError(
            "Cannot deserialize to Any. Use JSONValue for arbitrary JSON instead."
        )
    if cls is JSONValue:
        return value

    if typing.get_origin(cls) is dict:
        # dict values need to have a type
        type_args = typing.get_args(cls)
        if type_args:
            dict_key_type, dict_value_type = type_args
        else:
            raise TypeError(f"Cannot deserialize untyped Dicts: {cls}")

        # value also has to be a dict
        if type(value) is dict:
            result: dict[str, Any] = {}
            for key, dict_value in value.items():
                key = _parse_value_into(key, dict_key_type)
                result[key] = _parse_value_into(dict_value, dict_value_type)
            return result
        else:
            raise TypeError(
                f"cannot parse {cls} from a non-dict value of type: {type(value)}"
            )

    elif typing.get_origin(cls) is list:
        # List items need to have a type
        list_item_type = typing.get_args(cls)[0]
        # value also has to be a list
        if type(value) is list:
            return [_parse_value_into(item, list_item_type) for item in value]
        else:
            raise TypeError(
                f"cannot parse {cls} from a non-list value({type(value)})"
            )

    elif typing.get_origin(cls) is set:
        # Set items need to have a type
        set_item_type = typing.get_args(cls)[0]
        # the value for a Set type needs to be a List or a Set.
        if (type(value) is list) or (type(value) is set):
            return set(
                [_parse_value_into(item, set_item_type) for item in value]
            )
        else:
            raise TypeError(
                f"cannot parse {cls} from a non-list value ({type(value)})"
            )

    elif _has_field_types(cls):
        # Create an object from this value
        return instance_from_dict(cls, value)

    elif (
        typing.get_origin(cls) is Union
        or typing.get_origin(cls) is types.UnionType
    ):
        # Unions are special, because we don't know what value we can make, so
        # just try them all, in order, until we get one that works.
        errors: list[BaseException] = []
        type_options = [*typing.get_args(cls)]

        # Strip the Nonetype
        if types.NoneType in type_options:
            type_options.remove(types.NoneType)

        # There are some special types that can deserialize nearly anything,
        # like str, or are very general like dict, set, and list, so lets
        # extract those, and do them last.
        possible_special_types = [dict, set, list, str]
        special_types_to_try = []

        for special_type in possible_special_types:
            if special_type in type_options:
                type_options.remove(special_type)
                special_types_to_try.append(special_type)

        # Now try the "normal" types
        for type_arg in type_options:
            try:
                return _parse_value_into(value, type_arg)
            except KeyError as ke:
                errors.append(ke)
            except TypeError as te:
                errors.append(te)
            except ValueError as ve:
                errors.append(ve)

        # Now try the "special" types
        for type_arg in special_types_to_try:
            try:
                return _parse_value_into(value, type_arg)
            except KeyError as ke:
                errors.append(ke)
            except TypeError as te:
                errors.append(te)
            except ValueError as ve:
                errors.append(ve)

        raise TypeError(
            f"Unable to create an instance of {cls}, from {value}: {errors}"
        )

    else:
        # It's probably a simple type, so directly instantiate it
        return cls(value)


def _has_field_types(cls: type[Any]) -> bool:
    """Returns True if the class in question has member field type annotations.

    This is akin to [`typing.get_type_hints()`], except that this doesn't raise
    any errors on types that don't support annotations at all (like ['Union']).
    """
    for name, value in inspect.getmembers(cls):
        if name == "__annotations__":
            return True
    return False


####
#
#  Serialization Flow
#


def instance_to_dict(instance: Any) -> dict[str, Any]:
    """Convert the object to a dictionary of its fields, ready for serialization
    into JSON.

    This supports classes that use PEP-526 type annotations, and is meant to
    work especially well with classes that use [`@dataclass`], such as the
    following example::

        from dataclass import dataclass
        from typing import Optional
        @dataclass
        class FooClass:
            some_field: int
            some_other_field: Optional[str]
    """
    # Get the type hints for the class, not the instance, because it's only
    # meant to be used with 'function, method, module, or class object' per the
    # documentation.
    field_types = typing.get_type_hints(instance.__class__)
    result = {}
    for name in field_types.keys():
        # First get the value of each field.
        value = getattr(instance, name)
        if value is not None:
            # If a serializer fn was added via metadata, use that. Otherwise use
            # the "default" handler
            metadata: dict[str, Any] | None = getattr(
                instance.__class__, "__SERIALIZE_AS__", None
            )
            serializer: Callable[[Any], Any] | None = None
            if metadata:
                serializer = metadata.get(name)
            if serializer:
                result[name] = serializer(value)
            else:
                result[name] = make_dict_value_for(value)

    return result


def make_dict_value_for(obj: Any) -> dict[str, Any] | list[Any] | str | int:
    """Create the value to put into a dictionary for the given object."""
    if isinstance(obj, dict):
        # Dicts are special, and need to be treated individually.
        result = {}
        for key, value in obj.items():
            # Recurse for each value in the dict.
            result[str(key)] = make_dict_value_for(value)
        return result

    elif isinstance(obj, list):
        # Lists are special, they need to retain their existing order
        return [make_dict_value_for(value) for value in obj]

    elif isinstance(obj, set):
        # Sets are special, as they are serialized to a list, and lists are
        # order sensitive for compares, unlike sets.  To ensure that different
        # sets that contain the same elements always have the same serialized
        # representation the set's contents are sorted when creating the list.
        return [make_dict_value_for(value) for value in sorted(obj)]

    elif _has_field_types(type(obj)):
        # It's something else, and it has field type annotations, so let's use
        # those to get a dictionary.
        return instance_to_dict(obj)

    else:
        # It doesn't support type hints, so just use it as-is, and hope for the
        # best.
        return obj


####
#
#  JSON Deserialization
#


def json_loads(cls: type[C], s: str) -> C:
    """Deserialize an instance of type 'cls' from JSON in the string 's'.

    This supports classes that use PEP-526 type annotations, and is meant to
    work especially well with classes that use [`@dataclass`], such as the
    following example::

        from dataclass import dataclass
        from typing import Optional
        @dataclass
        class FooClass:
            some_field: int
            some_other_field: Optional[str]
    """
    return instance_from_dict(cls, json.loads(s))


def json_load(cls: type[C], fp: io.IOBase) -> C:
    """Deserialize an instance of type 'cls' from JSON read from a read()-
    supporting object.

    This supports classes that use PEP-526 type annotations, and is meant to
    work especially well with classes that use [`@dataclass`], such as the
    following example::

        from dataclass import dataclass
        from typing import Optional
        @dataclass
        class FooClass:
            some_field: int
            some_other_field: Optional[str]
    """
    return instance_from_dict(cls, json.load(fp))


####
#
#  JSON Serialization
#


def json_dump(instance: Any, fp: io.IOBase, **kwargs: Any) -> None:
    """Serialize an object into json written to a write()-supporting object.

    This supports classes that use PEP-526 type annotations, and is meant to
    work especially well with classes that use [`@dataclass`], such as the
    following example::

        from dataclass import dataclass
        from typing import Optional
        @dataclass
        class FooClass:
            some_field: int
            some_other_field: Optional[str]
    """
    json.dump(instance_to_dict(instance), fp, **kwargs)


def json_dumps(instance: Any, **kwargs: Any) -> str:
    """Serialize an object into json written to a string.

    This supports classes that use PEP-526 type annotations, and is meant to
    work especially well with classes that use [`@dataclass`], such as the
    following example::

        from dataclass import dataclass
        from typing import Optional
        @dataclass
        class FooClass:
            some_field: int
            some_other_field: Optional[str]
    """
    return json.dumps(instance_to_dict(instance), **kwargs)


####
#
#  Class Decorators for JSON and dict-based serialization
#


def serialize_dict(cls: type[C]) -> type[C]:
    """A decorator that adds dictionary-based serialization and deserialization
    fns to the class, which operate using the type annotations for the class's
    members and the params of the __init__() fn.

    Examines PEP 526 __annotations__ to determine how to serialize/deserialize
    the given class.

    Example:
    ```
    @dataclass
    @serialize_dict
    class MyClass:
        some_field: int
        another_field: string
    ```

    This decorator adds the following functions to the class definition:
    ```
    @classfunction
    def from_dict(cls: type[Self], value: Dict) -> Self:
      return serialization.instance_from_dict(cls, value)

    def to_dict(self) -> Dict:
      return serialization.instance_to_dict(self)
    ```

    Note: While the functions are added to the class, they aren't properly
    detected by linters such as PyRight, and so they will be flagged as issues
    when used.
    """

    def wrap(cls: type[C]) -> type[C]:
        _bind_class_fn(cls, instance_from_dict, "from_dict")
        _bind_instance_fn(cls, instance_to_dict, "to_dict")
        return cls

    return wrap(cls)


# WARNING: DEPRECATED: This decorator defeats mypy's static analysis and will
# result in typing errors at build time for any code that uses it. Use
# serialization.{json_load, json_loads, json_dump, json_dumps} directly instead.
def serialize_json(cls: type[C]) -> type[C]:
    """A decorator that adds JSON serialization and deserialization fns to the
    class, which follow the [`json.load()`], [`json.dumps()`], etc. functions.
    They operate using the type annotations for the class's members and the
    params of the __init__() fn.

    Examines PEP 526 __annotations__ to determine how to serialize/deserialize
    the given class.

    Example:
    ```
    @dataclass
    @serialize_json
    class MyClass:
        some_field: int
        another_field: string
    ```

    This decorator adds the following functions to the class definition:
    ```
    @classfunction
    def json_loads(cls: type[Self], value: str) -> Self:
      return serialization.json_loads(cls, value)

    @classfunction
    def json_load(cls: type[Self], fp: SupportsRead) -> Self:
      return serialization.json_load(cls, fp)

    def json_dumps(self, **kwargs) -> str:
      return serialization.json_dumps(self, **kwargs)

    def json_dump(self, fp: SupportsWrite, **kwargs) -> str:
      return serialization.json_dump(self, fp, **kwargs)
    ```

    Note: While the functions are added to the class, they aren't properly
    detected by linters such as PyRight, and so they will be flagged as issues
    when used.
    """

    def wrap(cls: type[C]) -> type[C]:
        _bind_class_fn(cls, json_load)
        _bind_class_fn(cls, json_loads)
        _bind_instance_fn(cls, json_dump)
        _bind_instance_fn(cls, json_dumps)
        return cls

    return wrap(cls)


def serialize_fields_as(**kwargs: Any) -> Callable[[type[C]], type[C]]:
    """Adds serialization metadata to the class, which is used to augment the
    PEP 526 __annotations__ to determine how to serialize the fields of the
    given class.

    Each is provided as a `fieldname=class` pair, or a `fieldname=fn` pair,
    which is called when serializing the field with that name.

    Example:
    ```
    def some_func(value: str) -> str:
        return f'serialized {value}.'

    @dataclass
    @serialize_json
    @serialize_fields_as(my_int_field=str,my_other_field=some_func)
    class MyClass:
        my_int_field: int
        my_other_field: str

    instance = MyClass(45, "hello")
    assertEqual(
        instance.json_dumps(),
        '{"my_int_field":"45","my_other_field":"serialized hello"}'
    )
    ```
    """

    def wrap(cls: type[C]) -> type[C]:
        return _process_metadata(cls, **kwargs)

    return wrap


####
#
#  Decorator Utility Functions
#


def _bind_class_fn(cls: type[C], fn: Any, name: str | None = None) -> None:
    """Creates a class-fn for a class by binding the passed-in class as the first
    param of the passed-in function, and then adding it as a callable attribute
    to the class.
    """
    if name is None:
        name = fn.__name__
    setattr(cls, name, functools.partial(fn, cls))


def _bind_instance_fn(cls: type[C], fn: Any, name: str | None = None) -> None:
    """Creates an instance-fn for a class by adding it as a callable attribute
    of the class.
    """
    if name is None:
        name = fn.__name__
    setattr(cls, name, fn)


def _process_metadata(cls: type[C], **kwargs: Any) -> type[C]:
    for name in kwargs.keys():
        if not hasattr(cls, name):
            annotations = get_type_hints(cls)
            if name not in annotations:
                raise ValueError(f"{cls} does not have a field named: {name}:")
    if kwargs:
        setattr(cls, "__SERIALIZE_AS__", kwargs)
    return cls
