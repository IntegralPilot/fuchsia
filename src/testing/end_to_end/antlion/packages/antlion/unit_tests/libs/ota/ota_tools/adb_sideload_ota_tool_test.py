#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
import os
import unittest

import mock
from antlion.controllers import android_device
from antlion.libs.ota.ota_runners import ota_runner
from antlion.libs.ota.ota_tools import adb_sideload_ota_tool, ota_tool


def get_mock_android_device(serial="", ssh_connection=None):
    """Returns a mocked AndroidDevice with a mocked adb/fastboot."""
    with mock.patch(
        "antlion.controllers.adb.AdbProxy"
    ) as adb_proxy, mock.patch(
        "antlion.controllers.fastboot.FastbootProxy"
    ) as fb_proxy:
        adb_proxy.return_value.getprop.return_value = "1.2.3"
        fb_proxy.return_value.devices.return_value = ""
        ret = mock.Mock(
            android_device.AndroidDevice(
                serial=serial, ssh_connection=ssh_connection
            )
        )
        fb_proxy.reset_mock()
        return ret


class AdbSideloadOtaToolTest(unittest.TestCase):
    """Tests the OtaTool class."""

    def test_init(self):
        expected_value = "commmand string"
        self.assertEqual(
            ota_tool.OtaTool(expected_value).command, expected_value
        )

    def setUp(self):
        self.sl4a_service_setup_time = ota_runner.SL4A_SERVICE_SETUP_TIME
        ota_runner.SL4A_SERVICE_SETUP_TIME = 0
        logging.log_path = "/tmp/log"

    def tearDown(self):
        ota_runner.SL4A_SERVICE_SETUP_TIME = self.sl4a_service_setup_time

    @staticmethod
    def test_start():
        # This test could have a bunch of verify statements,
        # but its probably not worth it.
        device = get_mock_android_device()
        ota_package_path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.realpath(__file__))),
            "dummy_ota_package.zip",
        )
        tool = adb_sideload_ota_tool.AdbSideloadOtaTool(ota_package_path)
        runner = ota_runner.SingleUseOtaRunner(
            tool, device, ota_package_path, ""
        )
        runner.android_device.adb.getprop = mock.Mock(side_effect=["a", "b"])
        runner.update()


if __name__ == "__main__":
    unittest.main()
