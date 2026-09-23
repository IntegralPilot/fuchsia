# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from dataclasses import dataclass
import enum
import typing
import unittest

from dataparse import dataparse
from dataparse import DataParseError

"""The following classes are for test purposes.

They describe a weather API reporting weather status and forecasts
for a specific city.
"""


@dataparse
@dataclass(frozen=True, order=True)
class City:
    name: str


@dataparse
@dataclass
class TemperatureDataPoint:
    temperature_celsius: float
    hour: int


@dataparse
@dataclass
class Weather:
    temperature_celsius: float
    chance_of_rain: float
    description: str
    city: City
    temperature_by_hour: list[TemperatureDataPoint] | None = None
    weather_station_ids: typing.Set[int] | None = None
    suggested_cities: typing.Set[City] | None = None


class TestDataParse(unittest.TestCase):
    def test_from_dict(self) -> None:
        """Test basic from_dict.

        This test instantiates a Weather object from a dictionary and asserts that
        the fields were correctly set.
        """

        weather: Weather = Weather.from_dict(  # type:ignore
            {
                "temperature_celsius": 12.0,
                "chance_of_rain": 33.0,
                "description": "scattered showers",
                "city": {"name": "San Francisco"},
            }
        )
        self.assertAlmostEqual(weather.temperature_celsius, 12)
        self.assertAlmostEqual(weather.chance_of_rain, 33)
        self.assertEqual(weather.description, "scattered showers")
        self.assertEqual(weather.city.name, "San Francisco")

    def test_to_dict(self) -> None:
        """Test basic to_dict.

        This test instantiates a Weather object and asserts that it can be
        converted to a dictionary.
        """

        weather = Weather(
            temperature_celsius=14,
            chance_of_rain=5,
            description="sunny",
            city=City(name="San Diego"),
        )
        d: dict[str, typing.Any] = weather.to_dict()  # type:ignore
        self.assertDictEqual(
            d,
            {
                "temperature_celsius": 14,
                "chance_of_rain": 5,
                "description": "sunny",
                "city": {"name": "San Diego"},
            },
        )

    def test_with_collections(self) -> None:
        """Test advanced collection to/from dict.

        This test instantiates a Weather object from a dictionary containing
        all of the different types supported by dataparse, including sets, lists,
        and sets/lists of @dataparse objects. It then converts to and from dicts
        to ensure that the data remains consistent and correct.
        """

        weather_dict = {
            "temperature_celsius": 12.0,
            "chance_of_rain": 33.25,
            "description": "scattered showers",
            "city": {"name": "San Francisco"},
            "temperature_by_hour": [
                {
                    "temperature_celsius": 11,
                    "hour": 13,
                },
                {
                    "temperature_celsius": 11.5,
                    "hour": 15,
                },
                {
                    "temperature_celsius": 12,
                    "hour": 16,
                },
                {
                    "temperature_celsius": 12.25,
                    "hour": 17,
                },
                {
                    "temperature_celsius": 13,
                    "hour": 18,
                },
                {
                    "temperature_celsius": 12,
                    "hour": 19,
                },
            ],
            "weather_station_ids": [1000, 1050, 2000],
            "suggested_cities": [{"name": "Oakland"}, {"name": "San Jose"}],
        }
        weather: Weather = Weather.from_dict(weather_dict)  # type:ignore
        hourly = weather.temperature_by_hour or []
        self.assertEqual(len(hourly), 6)
        self.assertIsInstance(hourly[0], TemperatureDataPoint)
        self.assertIsInstance(weather.suggested_cities, set)
        for v in weather.suggested_cities or set():
            self.assertIsInstance(v, City)
        self.assertSetEqual(
            weather.weather_station_ids or set(), {1000, 2000, 1050}
        )
        self.assertSetEqual(
            weather.suggested_cities or set(),
            set([City("Oakland"), City("San Jose")]),
        )
        self.assertDictEqual(weather_dict, weather.to_dict())  # type:ignore

    def test_unsortable_set(self) -> None:
        """Test dataparse with a set of unsortable values.

        Dataparse stores sets as lists in the output for better JSON
        serialization, and it attempts to sort the list before output.
        This test ensures that dataparse does not fail when the set's
        contents cannot be sorted.
        """

        @dataparse
        @dataclass(frozen=True)
        class Name:
            name: str

        @dataparse
        @dataclass
        class WithSet:
            set_of_ids: typing.Set[Name]

        val = WithSet(set_of_ids=set([Name("A"), Name("B")]))
        new_val: WithSet = WithSet.from_dict(val.to_dict())  # type:ignore
        self.assertSetEqual(
            set([v.name for v in val.set_of_ids]),
            set([v.name for v in new_val.set_of_ids]),
        )

    def test_renames(self) -> None:
        """Test that field renames are respected by to_dict and from_dict."""

        @dataparse
        @dataclass
        class KV:
            the_key: str
            the_value: str

            @classmethod
            def dataparse_renames(cls) -> dict[str, str]:
                return {"the_key": "The Key", "the_value": "The Value"}

        input = {"The Key": "foo", "The Value": "bar"}
        kv: KV = KV.from_dict(input)  # type:ignore
        self.assertEqual(kv.the_key, "foo")
        self.assertEqual(kv.the_value, "bar")

        out: dict[str, typing.Any] = kv.to_dict()  # type:ignore
        self.assertDictEqual(input, out)

    def test_nulls(self) -> None:
        """Test null handling for dataparse.

        None (JSON: null) values in the input are passed verbatim as
        values in from_dict, but they are omitted in to_dict.
        """

        @dataparse
        @dataclass
        class HasNull:
            value: int | None = None

        has_null: HasNull = HasNull.from_dict({"value": None})  # type:ignore
        self.assertEqual(has_null, HasNull())
        out_dict: dict[str, typing.Any] = has_null.to_dict()  # type:ignore
        # Omit nulls in output.
        self.assertFalse(hasattr(out_dict, "value"))


class TestDataParseErrors(unittest.TestCase):
    def test_unknown_union(self) -> None:
        """Test for failure when we do not expect the union type.

        dataparse only supports optional (Union[Any, None]) unions.
        Ensure that using a different union type raises a DataParseError.
        """

        @dataparse
        @dataclass
        class BadUnion:
            val: typing.Union[int, float]

        self.assertRaises(
            DataParseError,
            lambda: BadUnion.from_dict(  # type:ignore[attr-defined]
                {"val": 30}
            ),
        )

    def test_invalid_class(self) -> None:
        """Test that we cannot wrap a non-dataclass with dataparse."""

        def wrap_a_non_dataclass() -> None:
            @dataparse
            class Bad:
                pass

        self.assertRaises(DataParseError, wrap_a_non_dataclass)

    def test_missing_required_field(self) -> None:
        """Test that missing a required field raises DataParseError rather than TypeError."""
        self.assertRaises(
            DataParseError,
            lambda: City.from_dict({}),  # type: ignore[attr-defined]
        )

    def test_reversed_optional_union(self) -> None:
        """Test that Union[None, int] (or None | int) works correctly."""

        @dataparse
        @dataclass
        class ReversedOptional:
            val: typing.Union[None, int] = None

        obj = ReversedOptional.from_dict({"val": 42})  # type: ignore[attr-defined]
        self.assertEqual(obj.val, 42)


class StatusEnum(enum.Enum):
    OK = "OK"
    Failure = "FAILURE"


@dataparse
@dataclass
class ContainsEnum:
    status: StatusEnum


class TestDataParseEnums(unittest.TestCase):
    def test_valid_enum_type(self) -> None:
        """Test that we can serialize and deserialize enum values."""

        value = ContainsEnum(status=StatusEnum.Failure)
        value2 = ContainsEnum(status=StatusEnum.OK)

        value_dict = value.to_dict()  # type:ignore[attr-defined]
        self.assertEqual(value_dict["status"], "FAILURE")
        value2_dict = value2.to_dict()  # type:ignore[attr-defined]
        self.assertEqual(value2_dict["status"], "OK")

        self.assertEqual(
            value,
            ContainsEnum.from_dict(value_dict),  # type:ignore[attr-defined]
        )
        self.assertEqual(
            value2,
            ContainsEnum.from_dict(value2_dict),  # type:ignore[attr-defined]
        )

    def test_invalid_enum_parsing(self) -> None:
        """Test that we throw an error when we encounter an unexpected enum value."""
        value_to_parse = {"status": "NOT VALID"}
        self.assertRaises(
            DataParseError,
            lambda: ContainsEnum.from_dict(value_to_parse),  # type:ignore
        )
