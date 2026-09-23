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
from antlion.libs.ota.ota_tools import update_device_ota_tool


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


class UpdateDeviceOtaToolTest(unittest.TestCase):
    """Tests for UpdateDeviceOtaTool."""

    def setUp(self):
        self.sl4a_service_setup_time = ota_runner.SL4A_SERVICE_SETUP_TIME
        ota_runner.SL4A_SERVICE_SETUP_TIME = 0
        logging.log_path = "/tmp/log"

    def tearDown(self):
        ota_runner.SL4A_SERVICE_SETUP_TIME = self.sl4a_service_setup_time

    def test_update(self):
        ota_package_path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.realpath(__file__))),
            "dummy_ota_package.zip",
        )
        with mock.patch("tempfile.mkdtemp") as mkdtemp, mock.patch(
            "shutil.rmtree"
        ) as rmtree, mock.patch("antlion.utils.unzip_maintain_permissions"):
            mkdtemp.return_value = ""
            rmtree.return_value = ""
            device = get_mock_android_device()
            tool = update_device_ota_tool.UpdateDeviceOtaTool(ota_package_path)
            runner = mock.Mock(
                ota_runner.SingleUseOtaRunner(tool, device, "", "")
            )
            runner.return_value.android_device = device
            with mock.patch("antlion.libs.proc.job.run"):
                tool.update(runner)
            del tool

    def test_del(self):
        ota_package_path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.realpath(__file__))),
            "dummy_ota_package.zip",
        )
        with mock.patch("tempfile.mkdtemp") as mkdtemp, mock.patch(
            "shutil.rmtree"
        ) as rmtree, mock.patch("antlion.utils.unzip_maintain_permissions"):
            mkdtemp.return_value = ""
            rmtree.return_value = ""
            tool = update_device_ota_tool.UpdateDeviceOtaTool(ota_package_path)
            del tool
            self.assertTrue(mkdtemp.called)
            self.assertTrue(rmtree.called)


if __name__ == "__main__":
    unittest.main()
