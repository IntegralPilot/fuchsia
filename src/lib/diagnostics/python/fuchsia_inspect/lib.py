# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import typing
from dataclasses import dataclass


class InspectDataError(Exception):
    """Base class for errors reading inspect data"""


class VersionMismatchError(InspectDataError):
    """Raised when we receive an unexpected schema version"""


class InvalidDataTypeError(InspectDataError):
    """Raised when data of a type other than "Inspect" is read"""


class MissingFieldError(InspectDataError):
    """Raised when the data is missing an expected field"""


class InvalidFieldError(InspectDataError):
    """Raised when the data has a non-dictionary where a dictionary is expected"""


@dataclass
class InspectMetadataError:
    message: str

    def __str__(self) -> str:
        return self.message


class Timestamp:
    def __init__(self, timestamp_nanos: int):
        self._nanos: int = int(timestamp_nanos)

    def seconds(self) -> float:
        """The number of seconds represented by this timestamp.

        Returns:
            float: Timestamp in seconds, as a float.
        """
        return self._nanos / 1e9

    def nanoseconds(self) -> int:
        """The number of nanoseconds represented by this timestamp.

        Returns:
            int: Timestamp in nanoseconds, as an int.
        """
        return self._nanos


@dataclass
class InspectMetadata:
    timestamp: Timestamp
    file_name: str | None = None
    errors: list[str] | None = None
    component_url: str | None = None

    @staticmethod
    def from_dict(data: dict[str, typing.Any]) -> "InspectMetadata":
        """Process the dictionary as an InspectMetadata field.

        Args:
            data (dict[str, typing.Any]): Source dictionary

        Returns:
            InspectMetadata: Validated dictionary contents.

        Raises:
            InspectDataError: If data is missing required fields.
        """
        timestamp = Timestamp(int(_extract_or_throw(data, "timestamp")))

        return InspectMetadata(
            timestamp=timestamp,
            file_name=data.get("file_name"),
            errors=data.get("errors"),
            component_url=data.get("component_url"),
        )


@dataclass
class InspectData:
    moniker: str
    metadata: InspectMetadata
    payload: dict[str, typing.Any] | None
    version: int

    @staticmethod
    def from_dict(data: dict[str, typing.Any]) -> "InspectData":
        """Process the dictionary as InspectData.

        Args:
            data (dict[str, typing.Any]): Source dictionary.

        Raises:
            VersionMismatchError: If the version is unexpected.
            InvalidDataTypeError: If the data fails validation.
            InspectDataError: If the data is missing a field.

        Returns:
            InspectData: The parsed and validated contents.
        """
        version = int(_extract_or_throw(data, "version"))
        if version != 1:
            raise VersionMismatchError(f"Found version {version}, expected 1")

        data_source = str(_extract_or_throw(data, "data_source"))
        if data_source != "Inspect":
            raise InvalidDataTypeError(
                f"Expected Inspect data, found {data_source}"
            )

        return InspectData(
            version=version,
            moniker=str(_extract_or_throw(data, "moniker")),
            metadata=InspectMetadata.from_dict(
                _extract_or_throw(data, "metadata")
            ),
            payload=_extract_or_throw(data, "payload"),
        )


@dataclass
class InspectDataCollection:
    data: list[InspectData]

    @staticmethod
    def from_list(lst: list[dict[str, typing.Any]]) -> "InspectDataCollection":
        """Process a list into a collection.

        Args:
            lst (list[dict[str, typing.Any]]): Source list.

        Returns:
            InspectDataCollection: Validated and processed contents.
        """
        return InspectDataCollection([InspectData.from_dict(d) for d in lst])

    @staticmethod
    def from_json_list(json_str: str) -> "InspectDataCollection":
        """Process a string as JSON and turn into a collection.

        Args:
            json_str (str): Source JSON string.

        Returns:
            InspectDataCollection: Validated and processed contents.
        """
        return InspectDataCollection.from_list(json.loads(json_str))


def _extract_or_throw(
    data: typing.Dict[typing.Any, typing.Any], *path: str
) -> typing.Any:
    if len(path) == 0:
        raise ValueError("BUG: Path cannot be empty")
    next: typing.Any = data
    for p in path:
        if isinstance(next, dict):
            if p not in next:
                raise MissingFieldError(
                    f"While reading path {path}, expected to find {p} in {next}"
                )
            next = next.get(p)
        else:
            raise InvalidFieldError(
                f"While reading path {path}, expected to find a dictionary at {p} but found {next}"
            )

    return next
