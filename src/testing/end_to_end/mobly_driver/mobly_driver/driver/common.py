# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains common methods used by BaseDriver implementations."""

import json
from typing import Any

import yaml


class InvalidFormatException(Exception):
    """Raised when the file input provided to the Mobly Driver is invalid."""


class DriverException(Exception):
    """Raised when the Mobly Driver encounters an unexpected error."""


def read_yaml_from_file(filepath: str) -> Any:
    """Returns a Python object representing the |filepath|'s YAML content.

    Args:
      filepath: absolute path to file to read YAML from.

    Returns:
      A Python dictionary that represents the specified file's YAML content.

    Raises:
      InvalidFormatException if |filepath| does not contain valid YAML.
      IOError if failed to open file.
      OSError if filepath does not exist.
    """
    with open(filepath) as f:
        try:
            return yaml.load(f, Loader=yaml.SafeLoader)
        except yaml.YAMLError as e:
            raise InvalidFormatException(
                "Failed to parse local Mobly config YAML: %s" % e
            )


def read_json_from_file(filepath: str) -> Any:
    """Returns a Python object representing the |filepath|'s JSON content.

    Args:
      filepath: absolute path to file to read JSON from.

    Returns:
      A Python object that represents the specified file's JSON content.

    Raises:
      InvalidFormatException if |filepath| does not contain valid JSON.
      IOError if failed to open file.
      OSError if filepath does not exist.
    """
    with open(filepath) as f:
        try:
            return json.load(f)
        except json.JSONDecodeError as e:
            raise InvalidFormatException("Failed to parse Testbed JSON: %s" % e)
