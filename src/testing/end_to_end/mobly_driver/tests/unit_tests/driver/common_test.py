# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for mobly_driver/driver/common.py."""

import json
import unittest
from typing import Any
from unittest.mock import patch

import yaml
from mobly_driver.driver import common


class CommonTest(unittest.TestCase):
    """Common tests"""

    @patch("builtins.open")
    @patch("yaml.load")
    def test_read_yaml_from_file_success(self, *unused_args: Any) -> None:
        """Test case for YAML file read success case"""
        common.read_yaml_from_file("path/to/file")

    @patch("builtins.open", side_effect=IOError)
    def test_read_yaml_from_file_open_failure_raises_exception(
        self, *unused_args: Any
    ) -> None:
        """Test case for YAML file read file open failure"""
        with self.assertRaises(IOError):
            common.read_yaml_from_file("path/to/file")

    @patch("builtins.open")
    @patch("yaml.load", side_effect=yaml.YAMLError)
    def test_read_yaml_from_file_yaml_parse_failure_raises_exception(
        self, *unused_args: Any
    ) -> None:
        """Test case for YAML file read YAML parse error"""
        with self.assertRaises(common.InvalidFormatException):
            common.read_yaml_from_file("path/to/file")

    @patch("builtins.open")
    @patch("json.load")
    def test_read_json_from_file_success(self, *unused_args: Any) -> None:
        """Test case for JSON file read success"""
        common.read_json_from_file("path/to/file")

    @patch("builtins.open")
    def test_read_json_from_file_open_failure_raises_exception(
        self, mock_open: Any
    ) -> None:
        """Test case for JSON file read file open failure"""
        mock_open.side_effect = IOError
        with self.assertRaises(IOError):
            common.read_json_from_file("path/to/file")

    @patch("builtins.open")
    @patch("json.load", side_effect=json.JSONDecodeError("msg", "doc", 0))
    def test_read_json_from_file_json_parse_failure_raises_exception(
        self, *unused_args: Any
    ) -> None:
        """Test case for JSON file read JSON parse error"""
        with self.assertRaises(common.InvalidFormatException):
            common.read_json_from_file("path/to/file")
