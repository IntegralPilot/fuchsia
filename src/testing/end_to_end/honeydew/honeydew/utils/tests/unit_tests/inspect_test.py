# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.utils.inspect.py."""

import json
import unittest
from unittest import mock

from honeydew.transports.ffx import ffx
from honeydew.utils import inspect as inspect_utils


class InspectTests(unittest.TestCase):
    """Unit tests for honeydew.utils.inspect.Inspect."""

    def setUp(self) -> None:
        super().setUp()
        self.mock_ffx = mock.MagicMock(spec=ffx.FFX)
        self.inspect_obj = inspect_utils.Inspect(ffx=self.mock_ffx)

    def test_show_success(self) -> None:
        """Test case for Inspect.show() success case."""
        expected_output = [{"path": "a", "contents": {}}]
        self.mock_ffx.run.return_value = json.dumps(expected_output)

        # "test_selector" isn't real selector format. We just want to make
        # sure that same string shows up in the command. The mock doesn't care.
        self.assertEqual(
            self.inspect_obj.show(selector="test_selector"), expected_output
        )
        self.mock_ffx.run.assert_called_once_with(
            cmd=["--machine", "json", "inspect", "show", "test_selector"],
            log_output=False,
        )

    def test_show_invalid_json(self) -> None:
        """Test case for Inspect.show() when FFX returns invalid JSON."""
        self.mock_ffx.run.return_value = "Error: something went wrong"

        with self.assertRaises(json.JSONDecodeError):
            self.inspect_obj.show(selector="test_selector")

    def test_show_from_component_success(self) -> None:
        """Test case for Inspect.show_from_component() success case."""
        expected_output = [{"path": "a", "contents": {}}]
        self.mock_ffx.run.return_value = json.dumps(expected_output)

        self.assertEqual(
            self.inspect_obj.show_from_component(
                component_query="test_component"
            ),
            expected_output,
        )
        self.mock_ffx.run.assert_called_once_with(
            cmd=["--machine", "json", "inspect", "show", "test_component"],
            log_output=False,
        )

    def test_show_text_success(self) -> None:
        """Test case for Inspect.show_text() success case."""
        expected_output = "some text output"
        self.mock_ffx.run.return_value = expected_output

        self.assertEqual(
            self.inspect_obj.show_text(selector="test_selector"),
            expected_output,
        )
        self.mock_ffx.run.assert_called_once_with(
            cmd=["inspect", "show", "test_selector"],
            log_output=False,
        )


if __name__ == "__main__":
    unittest.main()
