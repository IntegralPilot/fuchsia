# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Test to get the list of WLAN PHY devices. This test will only fail if
wlandevicemonitor is not running.
"""

import logging

logger = logging.getLogger(__name__)

import fidl_fuchsia_wlan_device_service as fidl_wlan_device_service
import fuchsia_base_test
from honeydew.typing.custom_types import FidlEndpoint
from mobly import test_runner


class WlanCanaryTest(fuchsia_base_test.FuchsiaBaseTest):
    async def setup_class(self) -> None:
        await super().setup_class()
        self.wlan_device_monitor_proxy = (
            fidl_wlan_device_service.DeviceMonitorClient(
                self.dut.fuchsia_controller.connect_device_proxy(
                    FidlEndpoint(
                        "core/wlandevicemonitor",
                        "fuchsia.wlan.device.service.DeviceMonitor",
                    )
                )
            )
        )

    async def test_wlandevicemonitor_is_responsive(self) -> None:
        phy_list = (await self.wlan_device_monitor_proxy.list_phys()).phy_list
        logger.info(f"List of PHY IDs: {phy_list}")


if __name__ == "__main__":
    test_runner.main()
