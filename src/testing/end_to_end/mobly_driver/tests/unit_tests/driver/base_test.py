# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for mobly_driver/driver/base.py."""

import os
import unittest
from typing import Any
from unittest.mock import patch

from mobly_driver.driver import base
from parameterized import parameterized


class BaseMoblyDriverTest(unittest.TestCase):
    """Base Mobly Driver tests"""

    @parameterized.expand(
        [
            (
                "output_path specified",
                "/user/path",
                {
                    base.TEST_OUTDIR_ENV: "/env/path",
                },
                "/user/path",
            ),
            (
                "output_path not specified",
                None,
                {
                    base.TEST_OUTDIR_ENV: "/env/path",
                },
                "/env/path",
            ),
        ]
    )
    @patch.multiple(base.BaseDriver, __abstractmethods__=set())
    def test_init_success(
        self,
        unused_name: str,
        output_path: str,
        test_env: dict[str, str],
        expected_output_path: str,
        *unused_args: Any,
    ) -> None:
        """Test case for initialization success"""
        with patch.dict(os.environ, test_env, clear=True):
            d = base.BaseDriver(  # type: ignore[abstract]
                honeydew_config={},
                output_path=output_path,
            )
            self.assertEqual(d._output_path, expected_output_path)

    @patch.multiple(base.BaseDriver, __abstractmethods__=set())
    def test_init_invalid_environment_raises_exception(
        self, *unused_args: Any
    ) -> None:
        """Test case for initialization failure"""
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(KeyError):
                base.BaseDriver(honeydew_config={})  # type: ignore[abstract]
