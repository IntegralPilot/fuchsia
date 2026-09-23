# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Testing utilities for antlion tests of wlanix.
"""

import struct
from typing import Any, Sequence

import fidl_fuchsia_wlan_wlanix as fidl_wlanix
import fuchsia_base_test
import openwrt_access_point
from antlion import controllers
from antlion.controllers.access_point import AccessPoint
from antlion.controllers.pdu import PduDevice
from honeydew.typing.custom_types import FidlEndpoint
from mobly import signals
from mobly.asserts import assert_equal, fail
from mobly.records import TestResultRecord
from openwrt_access_point import OpenWrtAP

NL80211_ATTR_WIPHY = 1
NL80211_ATTR_IFINDEX = 3
NL80211_ATTR_IFNAME = 4
NL80211_ATTR_MAC = 6


def verify_new_interface_response(
    response_list: Sequence[fidl_wlanix.Nl80211Message],
) -> dict[int, bytes]:
    last_response_index = len(response_list) - 1
    for response_index, response in enumerate(response_list):
        if response.done:
            assert_equal(
                response_index,
                last_response_index,
                "Nl80211 DONE message before end of response",
            )
            break
        elif response.error:
            fail(
                "Received an error Nl80211 message type: %s",
                response.error,
            )
        elif not response.message:
            fail(
                "Received an unexpected Nl80211 message: %s",
                response,
            )

        assert response.message
        assert (
            response.message.payload is not None
        ), "MESSAGE must contain a payload"

        payload = bytes(response.message.payload)
        formatted_response_payload = [
            format(b, "#04x") for b in response.message.payload
        ]
        assert_equal(
            response.message.payload[0],
            7,
            f"Payload is not a NewInterface message: {formatted_response_payload}",
        )

        attrs = {}
        offset = 4  # Skip GenNetlink header
        while offset + 4 <= len(payload):
            nla_len, nla_type = struct.unpack_from("<HH", payload, offset)
            if nla_len < 4:
                break
            # The most significant bits are reserved for NLA_F_NESTED
            # and NLA_F_NET_BYTEORDER.
            nla_type &= 0x3FFF
            value = payload[offset + 4 : offset + nla_len]
            attrs[nla_type] = value
            # Move offset forward to the next 4-byte boundary.
            offset += (nla_len + 3) & ~3

        assert (
            NL80211_ATTR_IFINDEX in attrs
        ), f"Response missing attribute NL80211_ATTR_IFINDEX: {formatted_response_payload}"
        assert (
            NL80211_ATTR_WIPHY in attrs
        ), f"Response missing attribute NL80211_ATTR_WIPHY: {formatted_response_payload}"
        assert (
            NL80211_ATTR_IFNAME in attrs
        ), f"Response missing attribute NL80211_ATTR_IFNAME: {formatted_response_payload}"
        assert (
            NL80211_ATTR_MAC in attrs
        ), f"Response missing attribute NL80211_ATTR_MAC: {formatted_response_payload}"

        return attrs

    raise RuntimeError(
        f"Did not find an iface index in the response list: {response_list}"
    )


class WlanixBaseTestClass(fuchsia_base_test.FuchsiaBaseTest):
    wlanix_proxy: fidl_wlanix.WlanixClient

    async def setup_class(self) -> None:
        await super().setup_class()

        self.wlanix_proxy = fidl_wlanix.WlanixClient(
            self.dut.fuchsia_controller.connect_device_proxy(
                FidlEndpoint("core/wlanix", "fuchsia.wlan.wlanix.Wlanix")
            )
        )


class WifiChipBaseTestClass(WlanixBaseTestClass):
    chip_id: int
    wifi_chip_proxy: fidl_wlanix.WifiChipClient
    allow_ifaces_between_tests: bool

    def __init__(
        self,
        *args: Any,
        allow_ifaces_between_tests: bool = False,
        **kwargs: Any,
    ) -> None:
        super().__init__(*args, **kwargs)
        self.allow_ifaces_between_tests = allow_ifaces_between_tests

    async def setup_class(self) -> None:
        await super().setup_class()

        (
            proxy,
            server,
        ) = self.dut.fuchsia_controller.channel_create()
        self.wlanix_proxy.get_wifi(wifi=server.take())
        wifi_proxy = fidl_wlanix.WifiClient(proxy)

        response = (await wifi_proxy.get_chip_ids()).unwrap()
        assert (
            response.chip_ids is not None
        ), "Wifi.GetChipIds() response is missing a chip_ids value"
        assert_equal(
            len(response.chip_ids),
            1,
            "Wifi.GetChipIds() should return exactly one chip_id.",
        )

        self.chip_id = response.chip_ids[0]
        (
            proxy,
            server,
        ) = self.dut.fuchsia_controller.channel_create()
        (
            await wifi_proxy.get_chip(chip_id=self.chip_id, chip=server.take())
        ).unwrap()
        self.wifi_chip_proxy = fidl_wlanix.WifiChipClient(proxy)

    async def teardown_test(self) -> None:
        if not self.allow_ifaces_between_tests:
            response = (
                await self.wifi_chip_proxy.get_sta_iface_names()
            ).unwrap()
            assert (
                response.iface_names is not None
            ), "WifiChip.GetStaIfaceNames() response is missing an iface_names value"
            assert_equal(
                len(response.iface_names),
                0,
                "Every test should end with no ifaces.",
            )
        await super().teardown_test()


class IfaceBaseTestClass(WifiChipBaseTestClass):
    wifi_sta_iface_proxy: fidl_wlanix.WifiStaIfaceClient
    supplicant_sta_iface_proxy: fidl_wlanix.SupplicantStaIfaceClient
    iface_name: str

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, allow_ifaces_between_tests=True, **kwargs)

    async def setup_class(self) -> None:
        await super().setup_class()

        get_sta_iface_names_response = (
            await self.wifi_chip_proxy.get_sta_iface_names()
        ).unwrap()
        assert (
            get_sta_iface_names_response.iface_names is not None
        ), "WifiChip.GetStaIfaceNames() response is missing an iface_names value"
        assert_equal(
            len(get_sta_iface_names_response.iface_names),
            0,
            "WifiChip should have returned an empty list of iface names",
        )

        (
            proxy,
            server,
        ) = self.dut.fuchsia_controller.channel_create()
        (
            await self.wifi_chip_proxy.create_sta_iface(iface=server.take())
        ).unwrap()
        self.wifi_sta_iface_proxy = fidl_wlanix.WifiStaIfaceClient(proxy)

        get_name_response = (
            await self.wifi_sta_iface_proxy.get_name()
        ).unwrap()
        assert (
            get_name_response.iface_name is not None
        ), "WifiStaIface.GetName() response is missing an iface_name value"
        self.iface_name = get_name_response.iface_name

        (
            proxy,
            server,
        ) = self.dut.fuchsia_controller.channel_create()
        self.wlanix_proxy.get_supplicant(supplicant=server.take())
        supplicant_proxy = fidl_wlanix.SupplicantClient(proxy)

        (
            proxy,
            server,
        ) = self.dut.fuchsia_controller.channel_create()

        self.wlanix_proxy.get_nl80211(nl80211=server.take())
        self.nl80211_proxy = fidl_wlanix.Nl80211Client(proxy)

        (
            proxy,
            server,
        ) = self.dut.fuchsia_controller.channel_create()
        supplicant_proxy.add_sta_interface(
            iface=server.take(),
            iface_name=self.iface_name,
        )
        self.supplicant_sta_iface_proxy = fidl_wlanix.SupplicantStaIfaceClient(
            proxy
        )

    async def teardown_class(self) -> None:
        (
            await self.wifi_chip_proxy.remove_sta_iface(
                iface_name=self.iface_name
            )
        ).unwrap()

        get_sta_iface_names_response = (
            await self.wifi_chip_proxy.get_sta_iface_names()
        ).unwrap()
        assert (
            get_sta_iface_names_response.iface_names is not None
        ), "WifiChip.GetStaIfaceNames() response is missing an iface_names value"
        assert_equal(
            len(get_sta_iface_names_response.iface_names),
            0,
            "WifiChip should no longer contain the iface just removed",
        )
        await super().teardown_class()


class ConnectionBaseTestClass(IfaceBaseTestClass):
    __access_point: AccessPoint | OpenWrtAP | None
    pdu_devices: list[PduDevice] | None
    nl80211_proxy: fidl_wlanix.Nl80211Client

    def access_point(self) -> AccessPoint | OpenWrtAP:
        if self.__access_point is None:
            raise RuntimeError("Connection tests require an access point.")
        return self.__access_point

    async def setup_class(self) -> None:
        await super().setup_class()
        self.pdu_devices = None

        access_points = await self.register_controller(
            controllers.access_point,
            required=False,
        )
        openwrt_aps = await self.register_controller(
            openwrt_access_point,
            required=False,
        )

        if openwrt_aps:
            self.__access_point = openwrt_aps[0]
        elif access_points:
            self.__access_point = access_points[0]
        else:
            raise signals.TestAbortClass("Requires at least one access point")

        self.pdu_devices = await self.register_controller(
            controllers.pdu,
            # TODO(https://fxbug.dev/369159708) This should be required, but it inhibits
            # local testing when a PDU is not present.
            required=False,
        )

        ap = self.access_point()
        if isinstance(ap, AccessPoint):
            ap.stop_all_aps()

    async def setup_test(self) -> None:
        await super().setup_test()

    async def teardown_test(self) -> None:
        # Maintain the invariant that every test starts with no access points.
        ap = self.access_point()
        if isinstance(ap, OpenWrtAP):
            ap.download_logs(self.log_path)
        elif isinstance(ap, AccessPoint):
            ap.download_ap_logs(self.log_path)
            ap.stop_all_aps()
        # Ensure that our supplicant is fully disconnected.
        await self.supplicant_sta_iface_proxy.disconnect()
        await super().teardown_test()

    async def on_fail(self, record: TestResultRecord) -> None:
        """A function that is executed upon a test failure.

        Args:
        record: A copy of the test record for this test, containing all information of
            the test execution including exception objects.
        """
        await super().on_fail(record)
        # Maintain the invariant that every test starts with no access points.
        ap = self.access_point()
        if isinstance(ap, AccessPoint):
            ap.stop_all_aps()
