# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from dataclasses import dataclass, field
from typing import Any, List, Optional, Sequence

from dataclasses_json_lite import config, dataclass_json


class DataclassesJsonLite(unittest.TestCase):
    def test_simple_field(self) -> None:
        @dataclass_json()
        @dataclass
        class MyClass:
            number: int
            numbers: List[int]
            maybe_number: Optional[int] = None
            maybe_number2: int | None = None

        self.assertEquals(
            MyClass(
                number=12,
                numbers=[1, 2, 3],
                maybe_number=None,
                maybe_number2=None,
            ),
            MyClass.from_dict(dict(number=12, numbers=[1, 2, 3])),  # type: ignore[attr-defined]
        )
        self.assertEquals(
            MyClass(
                number=12, numbers=[1, 2, 3], maybe_number=13, maybe_number2=14
            ),
            MyClass.from_dict(  # type: ignore[attr-defined]
                dict(
                    number=12,
                    numbers=[1, 2, 3],
                    maybe_number=13,
                    maybe_number2=14,
                )
            ),
        )

    def test_rename(self) -> None:
        @dataclass_json()
        @dataclass
        class MyClass:
            number: int = field(metadata=config(field_name="n"))
            numbers: List[int] = field(metadata=config(field_name="ns"))
            numbers2: Sequence[int] = field(metadata=config(field_name="ns2"))

        self.assertEquals(
            MyClass(number=12, numbers=[1, 2, 3], numbers2=[2, 3]),
            MyClass.from_dict(dict(n=12, ns=[1, 2, 3], ns2=[2, 3])),  # type: ignore[attr-defined]
        )

    def test_sub_class(self) -> None:
        @dataclass_json()
        @dataclass
        class UserClass:
            name: str
            age: int

        @dataclass_json()
        @dataclass
        class MyClass:
            member: UserClass
            members: List[List[UserClass]]

        self.assertEquals(
            MyClass(
                member=UserClass(name="A", age=1),
                members=[[UserClass(name="B", age=2)]],
            ),
            MyClass.from_dict(  # type: ignore[attr-defined]
                dict(
                    member=dict(name="A", age=1),
                    members=[[dict(name="B", age=2)]],
                )
            ),
        )

    def test_decoder(self) -> None:
        @dataclass_json()
        @dataclass
        class UserClass:
            name: str
            age: int

        def decoder(json_data: List[Any]) -> UserClass:
            return UserClass(*json_data)

        @dataclass_json()
        @dataclass
        class MyClass:
            member: UserClass = field(metadata=config(decoder=decoder))
            other: UserClass = field(
                metadata=config(decoder=decoder, field_name="O")
            )

        self.assertEquals(
            MyClass(
                member=UserClass(name="A", age=1),
                other=UserClass(name="B", age=2),
            ),
            MyClass.from_dict(dict(member=["A", 1], O=["B", 2])),  # type: ignore[attr-defined]
        )

    def test_unknown_field(self) -> None:
        @dataclass_json()
        @dataclass
        class MyClass:
            age: int = field(metadata=config(field_name="AA"))

        with self.assertRaises(ValueError) as ctx:
            MyClass.from_dict(dict(age=1))  # type: ignore[attr-defined]
        self.assertIn("age", str(ctx.exception))

        with self.assertRaises(ValueError) as ctx:
            MyClass.from_dict(dict(banana=1))  # type: ignore[attr-defined]
            self.assertIn("banana", str(ctx.exception))

    def test_missing_field(self) -> None:
        @dataclass_json()
        @dataclass
        class MyClass:
            age: int
            town: str = "Paris"

        with self.assertRaises(TypeError) as ctx:
            MyClass.from_dict(dict())  # type: ignore[attr-defined]
        self.assertIn(
            "missing 1 required positional argument: 'age'", str(ctx.exception)
        )

    def test_composition(self) -> None:
        self.assertEquals(
            MyComposedClass.from_dict(  # type: ignore[attr-defined]
                dict(opt_sub=dict(list_sub=[]), list_sub=[dict(list_sub=[])])
            ),
            MyComposedClass(
                opt_sub=MyComposedClass(
                    list_sub=[],
                ),
                list_sub=[MyComposedClass(list_sub=[])],
            ),
        )


@dataclass_json()
@dataclass
class MyComposedClass:
    """This class needs to be in the module scope so that it can be resolved."""

    list_sub: List["MyComposedClass"]
    opt_sub: Optional["MyComposedClass"] = None


if __name__ == "__main__":
    unittest.main()
