# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Common definitions used in license processing"""

import json
from collections.abc import Callable
from typing import Any


class LicenseException(Exception):
    """Exception class for exceptions in the license processing pipeline"""

    def __init__(self, message: str, location: str = None):
        if location:
            message = f"Error: {message} at {location}"
        super().__init__(self, message)


class DictReader:
    """Helper class for reading keyed values from JSON dictionaries."""

    def __init__(self, dictionary: dict[str, Any], location):
        if not isinstance(dictionary, dict):
            raise LicenseException(
                f"Expected dict but {type(dictionary)}", location
            )
        self._dict = dictionary
        self._location = location

    def create_from_file(file_path):
        with open(file_path, "r") as input_file:
            return DictReader(json.load(input_file), f"{file_path}")

    def _key_location(self, key):
        return f"{self._location}.{key}"

    @property
    def location(self):
        return self._location

    def exists(self, key) -> bool:
        """Returns whether `key` exists in the dictionary."""
        return key in self._dict

    def get(
        self,
        key: str,
        expected_type: type = str,
        verify: Callable[[Any], str] = None,
    ):
        """Get the dictionary value by 'key'.

        Args:
            key: The key of the value.
            expected_type: The expected type of the value.
            verify: A function to verify the contents of the value.
                The function should return None if the value is verified or an error message str.
        """
        value = self.get_or(key, None, expected_type, verify)
        if value is None:
            raise LicenseException(
                f"Required key '{key}' is missing in dict '{self._dict}'",
                self.location,
            )
        return value

    def get_or(
        self,
        key,
        default: Any,
        expected_type: type = str,
        verify: Callable[[Any], str] = None,
        accept_none: bool = False,
    ):
        """Get the dictionary value by 'key' or fallback to a default value.

        Args:
            key: The key of the value.
            default: The value to return in case the key is not found.
            expected_type: The expected type of the value. Defaults to the type of the default value or str otherwise.
            verify: A function to verify the contents of the value.
                The function should return None if the value is verified or an error message str.
            accept_none: Whether None value is accepted.
        """
        if default is not None:
            expected_type = type(default)

        if key in self._dict:
            value = self._dict[key]
            if value is None and accept_none:
                return value
            if not isinstance(value, expected_type):
                raise LicenseException(
                    f"Expected value of type {expected_type} but got {type(value)}, value='{value}'",
                    self._key_location(key),
                )
            if verify:
                msg = verify(value)
                if msg:
                    raise LicenseException(
                        f"Unverified value '{value}': {msg}",
                        self._key_location(key),
                    )
            return value
        return default

    def get_reader(self, key):
        value = self.get(key, expected_type=dict)
        if isinstance(value, dict):
            return DictReader(value, self._key_location(key))
        raise LicenseException(
            f"Expected dict for '{key}' but found {type(value)}",
            self._key_location(key),
        )

    def get_readers_list(self, key, dedup=False):
        output = []
        for value in self.get_or(key, [], expected_type=list):
            if not isinstance(value, dict):
                raise LicenseException(
                    f"Expected dict values in list but found {type(value)}",
                    self._key_location(key),
                )
            output.append(value)
        # Workaround for b/248101373#comment11. Some SPDX producers
        # produce duplicate json elements with the same SPDX Ref Ids.
        if dedup:
            unique_output = {}
            for value in output:
                dedup_key = json.dumps(value)
                if dedup_key not in unique_output:
                    unique_output[dedup_key] = value
            output = list(unique_output.values())
        return [DictReader(v, self._key_location(key)) for v in output]

    def get_string_list(self, key) -> list[str]:
        return self.get_or(
            key, expected_type=list, default=[], verify=_verify_string_list
        )

    def get_string_set(self, key) -> set[str]:
        """Reads a list of strings and converts to a Set"""
        return set(self.get_string_list(key))

    def get_string_or_none(self, key) -> str:
        value = self.get_or(key, default=None, expected_type=str)
        # Special treatment for SPDX NOASSERTION value:
        if value == "NOASSERTION":
            value = None
        return value


def _verify_string_list(value):
    if not isinstance(value, list):
        return "Expected list value"
    for v in value:
        if not isinstance(v, str):
            return f"Expected str elements but got {type(v)}"
    return None


def trim_long_str_list(items: list, max_len: int) -> list:
    """If items is longer than max_len, will trim it and leave a tombstone message at the end"""
    l = len(items)
    if l > max_len:
        items = items[0:max_len]
        items.append(f"... ({l - max_len} more items)")
    return items
