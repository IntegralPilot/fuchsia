# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for UsbPowerHubUsingDmc implementation of UsbPowerHub interface.

Note - This test relies on DMC which is available in infra host machines and
thus can be run only in infra mode. In order to run this test, Fuchsia Device
needs to be connected to USB power hub in infra and not all fuchsia devices are
connected to a USB power hub.
"""

import asyncio
import logging

import fuchsia_base_test
from honeydew.auxiliary_devices.usb_power_hub import (
    usb_power_hub,
    usb_power_hub_using_dmc,
)
from mobly import expects, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class UsbPowerHubUsingDmcTest(fuchsia_base_test.FuchsiaBaseTest):
    """Mobly test for UsbPowerDmc implementation of UsbPower interface."""

    async def setup_class(self) -> None:
        """setup_class is called once before running tests."""
        await super().setup_class()

        _LOGGER.debug("Instantiating UsbPowerDmc module")
        self._usb_power_hub: usb_power_hub.UsbPowerHub = (
            usb_power_hub_using_dmc.UsbPowerHubUsingDmc(
                device_name=self.dut.device_name
            )
        )

    async def test_usb_power_hub_using_dmc(self) -> None:
        """Test case for UsbPowerHubUsingDmc.power_off and UsbPowerHubUsingDmc.power_on"""

        # TODO(https://fxbug.dev/431799077): The USB data connection currently does not drop on
        # Sorrel when the USB is powered off, so this test reboots the dut into fastboot instead.
        # We should go back to testing ths behavior while booted in Fuchsia once the bug is fixed
        await self.dut.fastboot.boot_to_fastboot_mode()
        try:
            self.dut.ffx.notify_intentional_disconnect()
            self._usb_power_hub.power_off()
            _LOGGER.info("Waiting 10 seconds for the usb to disconnect")
            await asyncio.sleep(10)
            expects.expect_false(
                await self.dut.fastboot.is_in_fastboot_mode(),
                "Fasboot device is still visible",
            )
        finally:
            self._usb_power_hub.power_on()
            await self.dut.fastboot.wait_for_fastboot_mode()
            try:
                # TODO(https://fxbug.dev/436414807): The `fastboot reboot` command sometimes
                # reports an error, despite the command actually succeeding. Once this is fixed,
                # we can remove the try/except block.
                await self.dut.fastboot.boot_to_fuchsia_mode()
            except:
                await self.dut.fastboot.wait_for_fuchsia_mode()
                await self.dut.wait_for_online()
                await self.dut.on_device_boot()


if __name__ == "__main__":
    test_runner.main()
