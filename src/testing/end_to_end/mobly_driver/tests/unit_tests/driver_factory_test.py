# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for Mobly driver's driver_factory.py."""

import os
import unittest
from typing import Any
from unittest import mock

from mobly_driver import driver_factory
from mobly_driver.api import api_infra
from mobly_driver.driver import base, common, infra, local
from parameterized import parameterized

_HONEYDEW_CONFIG: dict[str, Any] = {
    "transports": {
        "ffx": {
            "path": "/ffx/path",
            "subtools_search_path": "subtools/search/path",
        }
    }
}


class DriverFactoryTest(unittest.TestCase):
    """Driver Factory tests"""

    @parameterized.expand(
        [
            (
                "local_env",
                {
                    base.TEST_OUTDIR_ENV: "log/path",
                },
                local.LocalDriver,
            ),
            (
                "infra_env",
                {
                    api_infra.BOT_ENV_TESTBED_CONFIG: "botanist.json",
                    base.TEST_OUTDIR_ENV: "log/path",
                },
                infra.InfraDriver,
            ),
        ]
    )
    def test_get_driver_success(
        self,
        unused_name: str,
        test_env: dict[str, str],
        expected_driver_type: type,
    ) -> None:
        """Test case to ensure driver resolution success"""
        factory = driver_factory.DriverFactory(honeydew_config=_HONEYDEW_CONFIG)
        with mock.patch.dict(os.environ, test_env, clear=True):
            driver = factory.get_driver()
        self.assertEqual(type(driver), expected_driver_type)

    def test_get_driver_unexpected_env_raises_exception(self) -> None:
        """Test case to ensure exception is raised on unexpected env"""
        factory = driver_factory.DriverFactory(honeydew_config=_HONEYDEW_CONFIG)

        # Undefined "api_infra.BOT_ENV_TEST_OUTDIR".
        invalid_infra_env = {api_infra.BOT_ENV_TESTBED_CONFIG: "botanist.json"}
        with mock.patch.dict(os.environ, invalid_infra_env, clear=True):
            with self.assertRaises(common.DriverException):
                factory.get_driver()

    def test_get_driver_with_ssh_key_success(self) -> None:
        """Test case to ensure ssh_key is passed to LocalDriver"""
        ssh_key = "/path/to/ssh/key"
        factory = driver_factory.DriverFactory(
            honeydew_config=_HONEYDEW_CONFIG, ssh_key=ssh_key
        )
        test_env = {
            base.TEST_OUTDIR_ENV: "log/path",
        }
        with mock.patch.dict(os.environ, test_env, clear=True):
            driver = factory.get_driver()
        self.assertIsInstance(driver, local.LocalDriver)
        assert isinstance(driver, local.LocalDriver)
        self.assertEqual(driver._ssh_key, ssh_key)
