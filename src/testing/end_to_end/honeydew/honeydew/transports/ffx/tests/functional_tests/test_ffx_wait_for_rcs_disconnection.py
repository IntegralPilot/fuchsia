# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for FFX transport."""

import logging

import fuchsia_base_test
from mobly import test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

_REBOOT: list[str] = ["target", "reboot"]


class FFXWaitForRCSDisconnectionTests(fuchsia_base_test.FuchsiaBaseTest):
    """Test class to test FFX.wait_for_rcs_disconnection().

    This is included in a separate test class as it involves rebooting the
    device.
    """

    async def test_wait_for_rcs_connection(self) -> None:
        """Test case for FFX.wait_for_rcs_connection()."""
        self.dut.ffx.wait_for_rcs_connection()

        self.dut.ffx.notify_intentional_disconnect()
        self.dut.ffx.run(_REBOOT)

        process = self.dut.ffx.wait_for_rcs_disconnection()
        process.wait(timeout=60)

        self.dut.ffx.wait_for_rcs_connection()

        # Make the device ready after reboot
        await self.dut.on_device_boot()


if __name__ == "__main__":
    test_runner.main()
