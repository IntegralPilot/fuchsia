#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
import logging

from antlion.libs.ota.ota_tools.ota_tool import OtaTool

# OTA Packages can be upwards of 1 GB. This may take some time to transfer over
# USB 2.0.
PUSH_TIMEOUT = 10 * 60


class AdbSideloadOtaTool(OtaTool):
    """Updates an AndroidDevice using adb sideload."""

    def __init__(self, ignored_command):
        # "command" is ignored. The ACTS adb version is used to prevent
        # differing adb versions from constantly killing adbd.
        super(AdbSideloadOtaTool, self).__init__(ignored_command)

    def update(self, ota_runner):
        logging.info("Rooting adb")
        ota_runner.android_device.root_adb()
        logging.info("Rebooting to sideload")
        ota_runner.android_device.adb.reboot("sideload")
        ota_runner.android_device.adb.wait_for_sideload()
        logging.info("Sideloading ota package")
        package_path = ota_runner.get_ota_package()
        logging.info(f'Running adb sideload with package "{package_path}"')
        ota_runner.android_device.adb.sideload(
            package_path, timeout=PUSH_TIMEOUT
        )
        logging.info("Sideload complete. Waiting for device to come back up.")
        ota_runner.android_device.adb.wait_for_recovery()
        ota_runner.android_device.reboot(stop_at_lock_screen=True)
        logging.info("Device is up. Update complete.")
