#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import unittest
from dataclasses import dataclass, field

from serialization import (
    instance_from_dict,
    instance_to_dict,
    serialize_dict,
    serialize_fields_as,
    serialize_json,
)


class SerializeFieldsTest(unittest.TestCase):
    """Validate that fields of different kinds are properly serialized."""

    def test_serialize_simple_class(self) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            str_field: str

        instance = SimpleClass(42, "a string")
        self.assertEqual(
            instance_to_dict(instance),
            {"int_field": 42, "str_field": "a string"},
        )

    def test_deserialize_simple_class(self) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            str_field: str

        value = {"int_field": 42, "str_field": "a string"}
        self.assertEqual(
            instance_from_dict(SimpleClass, value), SimpleClass(42, "a string")
        )

    def test_deserialize_simple_class_with_default_value(self) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            str_field: str = "a default value"

        value = {"int_field": 42}
        self.assertEqual(
            instance_from_dict(SimpleClass, value),
            SimpleClass(42, "a default value"),
        )

    def test_deserialize_simple_class_with_default_factory(self) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            str_field: list[str] = field(default_factory=list)

        value = {"int_field": 42}
        self.assertEqual(
            instance_from_dict(SimpleClass, value), SimpleClass(42, [])
        )

    def test_serialize_int_value_0(self) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            str_field: str

        instance = SimpleClass(0, "a string")
        self.assertEqual(
            instance_to_dict(instance),
            {"int_field": 0, "str_field": "a string"},
        )

    def test_serialize_optional_field_with_value(self) -> None:
        @dataclass
        class SimpleClassWithOptionalField:
            int_field: int | None
            str_field: str

        instance = SimpleClassWithOptionalField(21, "some value")
        self.assertEqual(
            instance_to_dict(instance),
            {"int_field": 21, "str_field": "some value"},
        )

    def test_serialize_optional_field_without_value(self) -> None:
        @dataclass
        class SimpleClassWithOptionalField:
            int_field: int | None
            str_field: str

        instance = SimpleClassWithOptionalField(None, "some value")
        self.assertEqual(
            instance_to_dict(instance), {"str_field": "some value"}
        )

    def test_serialize_list_fields(self) -> None:
        @dataclass
        class SimpleClassWithList:
            int_field: list[int] = field(default_factory=list)
            str_field: str = "foo"

        instance = SimpleClassWithList([1, 2, 3, 4, 5])
        self.assertEqual(
            instance_to_dict(instance),
            {"int_field": [1, 2, 3, 4, 5], "str_field": "foo"},
        )

    def test_serialize_empty_list_fields(self) -> None:
        @dataclass
        class SimpleClassWithList:
            int_field: list[int] = field(default_factory=list)
            str_field: str = "foo"

        instance = SimpleClassWithList()
        self.assertEqual(
            instance_to_dict(instance), {"int_field": [], "str_field": "foo"}
        )

    def test_deserialize_list_fields(self) -> None:
        @dataclass
        class SimpleClassWithList:
            int_field: list[int] = field(default_factory=list)
            str_field: str = "foo"

        instance = SimpleClassWithList([1, 2, 3, 4, 5])
        self.assertEqual(
            instance_from_dict(
                SimpleClassWithList,
                {"int_field": [1, 2, 3, 4, 5], "str_field": "foo"},
            ),
            instance,
        )

    def test_deserialize_empty_list_fields(self) -> None:
        @dataclass
        class SimpleClassWithList:
            int_field: list[int] = field(default_factory=list)
            str_field: str = "foo"

        instance = SimpleClassWithList()
        self.assertEqual(
            instance_from_dict(
                SimpleClassWithList, {"int_field": [], "str_field": "foo"}
            ),
            instance,
        )

    def test_deserialize_missing_list_fields_empty(self) -> None:
        @dataclass
        class SimpleClassWithList:
            int_field: list[int] = field(default_factory=list)
            str_field: str = "foo"

        instance = SimpleClassWithList()
        self.assertEqual(
            instance_from_dict(SimpleClassWithList, {"str_field": "foo"}),
            instance,
        )

    def test_serialize_set_fields(self) -> None:
        # Note that this also tests that sets are serialized into sorted-order
        @dataclass
        class SimpleClassWithSet:
            int_field: set[int] = field(default_factory=set)
            str_field: str = "foo"

        instance = SimpleClassWithSet(set([5, 4, 3, 2, 1]))
        self.assertEqual(
            instance_to_dict(instance),
            {"int_field": [1, 2, 3, 4, 5], "str_field": "foo"},
        )

    def test_serialize_empty_set_fields(self) -> None:
        @dataclass
        class SimpleClassWithSet:
            int_field: set[int] = field(default_factory=set)
            str_field: str = "foo"

        instance = SimpleClassWithSet()
        self.assertEqual(
            instance_to_dict(instance), {"int_field": [], "str_field": "foo"}
        )

    def test_deserialize_set_fields(self) -> None:
        @dataclass
        class SimpleClassWithSet:
            int_field: set[int] = field(default_factory=set)
            str_field: str = "foo"

        instance = SimpleClassWithSet(set([5, 4, 3, 2, 1]))
        self.assertEqual(
            instance_from_dict(
                SimpleClassWithSet,
                {"int_field": [1, 2, 3, 4, 5], "str_field": "foo"},
            ),
            instance,
        )

    def test_deserialize_empty_set_fields(self) -> None:
        @dataclass
        class SimpleClassWithSet:
            int_field: set[int] = field(default_factory=set)
            str_field: str = "foo"

        instance = SimpleClassWithSet()
        self.assertEqual(
            instance_from_dict(
                SimpleClassWithSet, {"int_field": [], "str_field": "foo"}
            ),
            instance,
        )

    def test_deserialize_missing_set_fields_empty(self) -> None:
        @dataclass
        class SimpleClassWithSet:
            str_field: str = "foo"
            int_field: set[int] = field(default_factory=set)

        instance = SimpleClassWithSet()
        self.assertEqual(
            instance_from_dict(SimpleClassWithSet, {"str_field": "foo"}),
            instance,
        )

    def test_serialize_dict_fields(self) -> None:
        @dataclass
        class SimpleClassWithDict:
            dict_field: dict[str, int]

        instance = SimpleClassWithDict({"one": 1, "two": 2, "three": 3})
        self.assertEqual(
            instance_to_dict(instance),
            {"dict_field": {"one": 1, "two": 2, "three": 3}},
        )

    def test_serialize_fields_as(self) -> None:
        @dataclass
        @serialize_fields_as(int_field=str)
        class SimpleClassWithMetdata:
            int_field: int
            str_field: str

        instance = SimpleClassWithMetdata(7, "a string")
        self.assertEqual(
            instance_to_dict(instance),
            {
                "int_field": "7",
                "str_field": "a string",
            },
        )

    def test_serialize_fields_as_with_callable(self) -> None:
        def my_serializer(value: int) -> str:
            return f"The value is {value}."

        @dataclass
        @serialize_fields_as(int_field=my_serializer)
        class SimpleClassWithMetdata:
            int_field: int
            str_field: str

        instance = SimpleClassWithMetdata(7, "a string")
        self.assertEqual(
            instance_to_dict(instance),
            {
                "int_field": "The value is 7.",
                "str_field": "a string",
            },
        )

    def test_serialize_class_with_superclass(self) -> None:
        @dataclass
        class SimpleBaseClass:
            int_field_base: int
            str_field_base: str

        @dataclass
        class SimpleChildClass(SimpleBaseClass):
            int_field_child: int
            str_field_child: str

        instance = SimpleChildClass(
            int_field_base=42,
            str_field_base="base",
            int_field_child=84,
            str_field_child="child",
        )
        self.assertEqual(
            instance_to_dict(instance),
            {
                "int_field_base": 42,
                "str_field_base": "base",
                "int_field_child": 84,
                "str_field_child": "child",
            },
        )

    def test_deserialize_class_with_superclass(self) -> None:
        @dataclass
        class SimpleBaseClass:
            int_field_base: int
            str_field_base: str

        @dataclass
        class SimpleChildClass(SimpleBaseClass):
            int_field_child: int
            str_field_child: str

        instance = SimpleChildClass(
            int_field_base=42,
            str_field_base="base",
            int_field_child=84,
            str_field_child="child",
        )

        self.assertEqual(
            instance_from_dict(
                SimpleChildClass,
                {
                    "int_field_base": 42,
                    "str_field_base": "base",
                    "int_field_child": 84,
                    "str_field_child": "child",
                },
            ),
            instance,
        )

    def test_serialize_class_with_multiple_superclasses(self) -> None:
        @dataclass
        class RootClass:
            int_field_root: int

        @dataclass
        class SimpleBaseClass(RootClass):
            int_field_base: int
            str_field_base: str

        @dataclass
        class AnotherBaseClass:
            str_field_another: str

        @dataclass
        class SimpleChildClass(SimpleBaseClass, AnotherBaseClass):
            int_field_child: int
            str_field_child: str

        instance = SimpleChildClass(
            int_field_root=89,
            int_field_base=42,
            str_field_base="base",
            str_field_another="another",
            int_field_child=84,
            str_field_child="child",
        )

        self.assertEqual(
            instance_to_dict(instance),
            {
                "int_field_root": 89,
                "int_field_base": 42,
                "str_field_base": "base",
                "str_field_another": "another",
                "int_field_child": 84,
                "str_field_child": "child",
            },
        )

    def test_deserialize_class_with_multiple_superclasses(self) -> None:
        @dataclass
        class RootClass:
            int_field_root: int

        @dataclass
        class SimpleBaseClass(RootClass):
            int_field_base: int
            str_field_base: str

        @dataclass
        class AnotherBaseClass:
            str_field_another: str

        @dataclass
        class SimpleChildClass(SimpleBaseClass, AnotherBaseClass):
            int_field_child: int
            str_field_child: str

        instance = SimpleChildClass(
            int_field_root=89,
            int_field_base=42,
            str_field_base="base",
            str_field_another="another",
            int_field_child=84,
            str_field_child="child",
        )

        self.assertEqual(
            instance_from_dict(
                SimpleChildClass,
                {
                    "int_field_root": 89,
                    "int_field_base": 42,
                    "str_field_base": "base",
                    "str_field_another": "another",
                    "int_field_child": 84,
                    "str_field_child": "child",
                },
            ),
            instance,
        )

    def test_deserialize_class_with_union_fields_first_type(self) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            union_field: int | list[str]

        self.assertEqual(
            SimpleClass(42, 23),
            instance_from_dict(
                SimpleClass, {"int_field": 42, "union_field": 23}
            ),
        )

    def test_deserialize_class_with_union_fields_second_type(self) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            union_field: int | list[str]

        self.assertEqual(
            SimpleClass(42, ["23"]),
            instance_from_dict(
                SimpleClass, {"int_field": 42, "union_field": ["23"]}
            ),
        )

    def test_deserialize_class_with_optional_fields_new_syntax_with_value(
        self,
    ) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            str_field: str | None = None

        self.assertEqual(
            SimpleClass(42, "foo"),
            instance_from_dict(
                SimpleClass, {"int_field": 42, "str_field": "foo"}
            ),
        )

    def test_deserialize_class_with_optional_fields_new_syntax_without_value(
        self,
    ) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            str_field: str | None = None

        self.assertEqual(
            SimpleClass(42),
            instance_from_dict(SimpleClass, {"int_field": 42}),
        )

    def test_deserialize_class_with_optional_fields_new_syntax_with_explicit_none_value(
        self,
    ) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            str_field: str | None = None

        self.assertEqual(
            SimpleClass(42),
            instance_from_dict(
                SimpleClass, {"int_field": 42, "str_field": None}
            ),
        )

        # Test when the field is not present
        self.assertEqual(
            SimpleClass(42),
            instance_from_dict(SimpleClass, {"int_field": 42}),
        )

    def test_deserialize_class_with_union_fields_new_syntax_first_type(
        self,
    ) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            union_field: str | list[str]

        self.assertEqual(
            SimpleClass(42, "foo"),
            instance_from_dict(
                SimpleClass, {"int_field": 42, "union_field": "foo"}
            ),
        )

    def test_deserialize_class_with_union_fields_new_syntax_second_type(
        self,
    ) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            union_field: int | list[str]

        self.assertEqual(
            SimpleClass(42, ["foo"]),
            instance_from_dict(
                SimpleClass, {"int_field": 42, "union_field": ["foo"]}
            ),
        )

    def test_deserialize_class_with_union_fields_mixed_syntax_second_type(
        self,
    ) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            union_field: str | list[str]

        self.assertEqual(
            SimpleClass(42, ["foo"]),
            instance_from_dict(
                SimpleClass, {"int_field": 42, "union_field": ["foo"]}
            ),
        )

    def test_deserialize_class_with_optional_union_fields_new_syntax_first_type(
        self,
    ) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            union_field: str | list[int] | None = None

        self.assertEqual(
            SimpleClass(42, "foo"),
            instance_from_dict(
                SimpleClass, {"int_field": 42, "union_field": "foo"}
            ),
        )

    def test_deserialize_class_with_optional_union_fields_new_syntax_second_type(
        self,
    ) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            union_field: list[int] | str | None = None

        self.assertEqual(
            SimpleClass(42, [23]),
            instance_from_dict(
                SimpleClass, {"int_field": 42, "union_field": [23]}
            ),
        )

    def test_deserialize_class_with_optional_union_fields_new_syntax_with_no_value(
        self,
    ) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            union_field: str | list[int] | None = None

        self.assertEqual(
            SimpleClass(42),
            instance_from_dict(SimpleClass, {"int_field": 42}),
        )

    def test_deserialize_class_with_optional_union_fields_new_syntax_with_explicit_none_value(
        self,
    ) -> None:
        @dataclass
        class SimpleClass:
            int_field: int
            union_field: str | list[int] | None = None

        self.assertEqual(
            SimpleClass(42),
            instance_from_dict(
                SimpleClass, {"int_field": 42, "union_field": None}
            ),
        )

    def test_serialize_nested_classes(self) -> None:
        @dataclass
        class Inner:
            int_field: int

        @dataclass
        class Outer:
            inner: Inner

        self.assertEqual(
            {"inner": {"int_field": 43}},
            instance_to_dict(Outer(Inner(43))),
        )

    def test_deserialize_nested_classes(self) -> None:
        @dataclass
        class Inner:
            int_field: int

        @dataclass
        class Outer:
            inner: Inner

        self.assertEqual(
            Outer(Inner(43)),
            instance_from_dict(Outer, {"inner": {"int_field": 43}}),
        )


class SerializeToDictDecorator(unittest.TestCase):
    """Validate that the `@serialize_to_dict class decorator behaves correctly.

    Note: These function correctly at runtime, but don't interact correctly with
    PyRight, so they don't have proper static analysis and IDE type-checking.
    """

    def test_to_dict_decorator(self) -> None:
        @dataclass
        @serialize_dict
        class SimpleClass:
            int_field: int
            str_field: str

        instance = SimpleClass(8, "some value")
        self.assertEqual(
            instance.to_dict(), {"int_field": 8, "str_field": "some value"}  # type: ignore[attr-defined]
        )

    def test_from_dict_decorator(self) -> None:
        @dataclass
        @serialize_dict
        class SimpleClass:
            int_field: int
            str_field: str

        raw = {"int_field": 8, "str_field": "some value"}
        self.assertEqual(
            SimpleClass.from_dict(raw),  # type: ignore[attr-defined]
            SimpleClass(8, "some value"),
        )


class SerializeToJsonDecorator(unittest.TestCase):
    """Validate that the `@serialize_to_json class decorator behaves correctly.

    Note: These function correctly at runtime, but don't interact correctly with
    PyRight, so they don't have proper static analysis and IDE type-checking.
    """

    def test_to_json_decorator(self) -> None:
        @dataclass
        @serialize_json
        class SimpleClass:
            int_field: int
            str_field: str

        instance = SimpleClass(8, "some value")
        result = instance.json_dumps(indent=6)  # type: ignore[attr-defined]
        self.assertEqual(
            result,
            """{
      "int_field": 8,
      "str_field": "some value"
}""",
        )

    def test_from_json_decorator(self) -> None:
        @dataclass
        @serialize_json
        class SimpleClass:
            int_field: int
            str_field: str

        raw = {"int_field": 8, "str_field": "some value"}
        raw_json = json.dumps(raw)

        self.assertEqual(
            SimpleClass.json_loads(raw_json), SimpleClass(8, "some value")  # type: ignore[attr-defined]
        )

    def test_to_json_decorator_with_field_serializer(self) -> None:
        def my_serializer(value: int) -> str:
            return f"my value is {value}."

        @dataclass
        @serialize_fields_as(int_field=my_serializer)
        @serialize_json
        class SimpleClass:
            int_field: int
            str_field: str

        instance = SimpleClass(8, "some value")
        result = instance.json_dumps(indent=6)  # type: ignore[attr-defined]
        self.assertEqual(
            result,
            """{
      "int_field": "my value is 8.",
      "str_field": "some value"
}""",
        )
