#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
import logging
import os
import shutil
import tempfile

from antlion import utils
from antlion.libs.ota.ota_tools import ota_tool
from libs.proc import job

# OTA Packages can be upwards of 1 GB. This may take some time to transfer over
# USB 2.0. A/B devices must also complete the update in the background.
UPDATE_TIMEOUT = 60 * 60
UPDATE_LOCATION = "/data/ota_package/update.zip"


class UpdateDeviceOtaTool(ota_tool.OtaTool):
    """Runs an OTA Update with system/update_engine/scripts/update_device.py."""

    def __init__(self, command):
        super(UpdateDeviceOtaTool, self).__init__(command)

        self.unzip_path = tempfile.mkdtemp()
        utils.unzip_maintain_permissions(self.command, self.unzip_path)

        self.command = os.path.join(self.unzip_path, "update_device.py")

    def update(self, ota_runner):
        logging.info("Forcing adb to be in root mode.")
        ota_runner.android_device.root_adb()
        update_command = "python3 %s -s %s %s" % (
            self.command,
            ota_runner.serial,
            ota_runner.get_ota_package(),
        )
        logging.info(f"Running {update_command}")
        result = job.run(update_command, timeout_sec=UPDATE_TIMEOUT)
        logging.info(f'Output: {result.stdout.decode("utf-8")}')

        logging.info("Rebooting device for update to go live.")
        ota_runner.android_device.reboot(stop_at_lock_screen=True)
        logging.info("Reboot sent.")

    def __del__(self):
        """Delete the unzipped update_device folder before ACTS exits."""
        shutil.rmtree(self.unzip_path)
