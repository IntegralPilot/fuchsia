# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for hello_world_using_ffx.py."""

import unittest
from collections.abc import Callable
from unittest import mock

from honeydew.affordances.hello_world import errors as hello_world_errors
from honeydew.affordances.hello_world import hello_world_using_ffx
from honeydew.transports.ffx import ffx as ffx_transport
from parameterized import param, parameterized

_TARGET_NAME: str = "fuchsia-emulator"


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_obj: param
) -> str:
    """Custom test name function method."""
    test_func_name: str = testcase_func.__name__
    test_label: str = parameterized.to_safe_name(param_obj.kwargs["label"])
    return f"{test_func_name}_with_{test_label}"


class HelloWorldUsingFfxTests(unittest.TestCase):
    """Unit tests for the hello_world_using_ffx.py."""

    def setUp(self) -> None:
        super().setUp()

        self.ffx_obj = mock.MagicMock(spec=ffx_transport.FFX)
        self.hello_world_obj = hello_world_using_ffx.HelloWorldUsingFfx(
            device_name=_TARGET_NAME, ffx=self.ffx_obj
        )

    @parameterized.expand(
        [
            param(
                label="name_arg_not_set",
                name=None,
                expected_greeting=f"Hello, {_TARGET_NAME}!",
            ),
            param(
                label="name_arg_set",
                name="starnix",
                expected_greeting="Hello, starnix!",
            ),
        ],
        name_func=_custom_test_name_func,
    )
    def test_greeting(
        self,
        label: str,  # pylint: disable=unused-argument
        name: str,
        expected_greeting: str,
    ) -> None:
        """Test for HelloWorldUsingFfx.greeting() method."""
        self.ffx_obj.get_target_name.return_value = _TARGET_NAME

        self.assertEqual(
            self.hello_world_obj.greeting(name=name), expected_greeting
        )

    def test_greeting_error(self) -> None:
        self.ffx_obj.get_target_name.return_value = _TARGET_NAME
        with self.assertRaises(hello_world_errors.HelloWorldAffordanceError):
            self.hello_world_obj.greeting(name=_TARGET_NAME)


if __name__ == "__main__":
    unittest.main()
