# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Test that the device has at least one WifiChip.
"""

import fidl_fuchsia_wlan_wlanix as fidl_wlanix
from mobly import test_runner
from mobly.asserts import assert_greater, assert_is_not
from wlanix_testing import base_test


class WifiChipExistenceTest(base_test.WlanixBaseTestClass):
    async def test_get_chip_ids(self) -> None:
        (
            proxy,
            server,
        ) = self.dut.fuchsia_controller.channel_create()
        self.wlanix_proxy.get_wifi(wifi=server.take())
        wifi_proxy = fidl_wlanix.WifiClient(proxy)

        response = (await wifi_proxy.get_chip_ids()).unwrap()
        assert_is_not(
            response.chip_ids,
            None,
            "Wifi.GetChipIds() response is missing a chip_ids value",
        )
        assert (
            response.chip_ids is not None
        ), "Wifi.GetChipIds() response is missing chip_ids"
        assert_greater(len(response.chip_ids), 0)


if __name__ == "__main__":
    test_runner.main()
