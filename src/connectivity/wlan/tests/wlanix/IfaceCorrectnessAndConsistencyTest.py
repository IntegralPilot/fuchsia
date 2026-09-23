# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Tests of various (sometimes hardcoded) properties of ifaces.
"""

import struct

import fidl_fuchsia_wlan_wlanix as fidl_wlanix
from mobly import test_runner
from mobly.asserts import assert_equal
from wlanix_testing import base_test


class IfaceCorrectnessAndConsistencyTest(base_test.IfaceBaseTestClass):
    # TODO(https://fxbug.dev/368005870): Need to reconsider the consequences of using
    # the same iface name, even when an iface is recreated.
    async def test_iface_name_hardcoded_as_wlan(self) -> None:
        response = (await self.wifi_sta_iface_proxy.get_name()).unwrap()
        assert_equal(
            response.iface_name,
            "wlan",
            'WifiStaIface should always return the hardcoded iface name "wlan"',
        )

    async def test_iface_name_consistency(self) -> None:
        get_sta_iface_names_response = (
            await self.wifi_chip_proxy.get_sta_iface_names()
        ).unwrap()
        assert get_sta_iface_names_response.iface_names is not None
        assert_equal(
            len(get_sta_iface_names_response.iface_names),
            1,
            "WifiChip should have returned the iface just created",
        )

        iface_name = get_sta_iface_names_response.iface_names[0]
        get_name_response = (
            await self.wifi_sta_iface_proxy.get_name()
        ).unwrap()
        assert_equal(
            iface_name,
            get_name_response.iface_name,
            "WifiStaIface returns a different name than WifiChip",
        )

    async def test_iface_nl80211_fields(self) -> None:
        """Verifies that NL80211_CMD_GET_INTERFACE returns correct fields."""
        get_interface_message = fidl_wlanix.Nl80211Message(
            message=fidl_wlanix.Message(
                # fmt: off
                payload=[
                    # Generic Netlink Header
                    0x05,  # Command: GetInterface
                    0x01,  # Version
                    0x00, 0x00 # Reserved
                ],
                # fmt: on
            )
        )
        message_response = (
            await self.nl80211_proxy.message(message=get_interface_message)
        ).unwrap()
        assert message_response.responses is not None
        attrs = base_test.verify_new_interface_response(
            list(message_response.responses)
        )

        # Verify fields
        ifIndex = struct.unpack("<I", attrs[base_test.NL80211_ATTR_IFINDEX])[0]
        assert ifIndex >= 0, f"Invalid ifIndex: {ifIndex}"

        wiphyIndex = struct.unpack("<I", attrs[base_test.NL80211_ATTR_WIPHY])[0]
        assert wiphyIndex >= 0, f"Invalid wiphyIndex: {wiphyIndex}"

        ifName = (
            attrs[base_test.NL80211_ATTR_IFNAME].decode("utf-8").rstrip("\x00")
        )
        assert_equal(ifName, "wlan", 'ifName should be "wlan"')

        macAddress = attrs[base_test.NL80211_ATTR_MAC]
        assert_equal(
            len(macAddress), 6, f"MAC address length invalid: {len(macAddress)}"
        )


if __name__ == "__main__":
    test_runner.main()
