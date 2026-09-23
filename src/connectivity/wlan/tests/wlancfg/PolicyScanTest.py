#!/usr/bin/env python3.4
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#

import dataclasses
import logging

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion.controllers.ap_lib import (
    hostapd_ap_preset,
    hostapd_bss_settings,
    hostapd_constants,
    hostapd_security,
)
from antlion.controllers.ap_lib.hostapd_security import SecurityMode
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    DEFAULT_5G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    SecurityOpen,
    SecurityWpa2,
)


@dataclasses.dataclass(frozen=True)
class NetworkInfo:
    ssid: str
    security: f_wlan_policy.SecurityType
    password: str | None = None


AP_SSID_LENGTH = 8
AP_PASSPHRASE_LENGTH = 10


class PolicyScanTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """WLAN policy scan test class.

    This test exercises the scan functionality for the WLAN Policy API.

    Test Bed Requirement:
    * One or more Fuchsia devices
    * One Whirlwind Access Point
    """

    async def setup_class(self) -> None:
        await super().setup_class()
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )
        self.log = logging.getLogger()

        if not self.openwrt_aps and not self.access_points:
            raise signals.TestAbortClass(
                "Requires at least one access point and one Fuchsia device"
            )

        if self.access_point:
            self.access_point.stop_all_aps()

        # Generate network params.
        self.open_network_2g = NetworkInfo(
            ssid=AccessPointConfig.random_string(AP_SSID_LENGTH),
            security=f_wlan_policy.SecurityType.NONE,
        )
        self.wpa2_network_2g = NetworkInfo(
            ssid=AccessPointConfig.random_string(AP_SSID_LENGTH),
            security=f_wlan_policy.SecurityType.WPA2,
            password=AccessPointConfig.random_string(AP_PASSPHRASE_LENGTH),
        )
        self.open_network_5g = NetworkInfo(
            ssid=AccessPointConfig.random_string(AP_SSID_LENGTH),
            security=f_wlan_policy.SecurityType.NONE,
        )
        self.wpa2_network_5g = NetworkInfo(
            ssid=AccessPointConfig.random_string(AP_SSID_LENGTH),
            security=f_wlan_policy.SecurityType.WPA2,
            password=AccessPointConfig.random_string(AP_PASSPHRASE_LENGTH),
        )

        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_2G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=self.open_network_2g.ssid,
                                security=SecurityOpen(),
                            ),
                            BssSettings(
                                ssid=self.wpa2_network_2g.ssid,
                                security=SecurityWpa2(),
                                password=self.wpa2_network_2g.password,
                            ),
                        ],
                    ),
                    RadioConfig.generate(
                        channel=DEFAULT_5G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=self.open_network_5g.ssid,
                                security=SecurityOpen(),
                            ),
                            BssSettings(
                                ssid=self.wpa2_network_5g.ssid,
                                security=SecurityWpa2(),
                                password=self.wpa2_network_5g.password,
                            ),
                        ],
                    ),
                ]
            )
            self.openwrt_ap.configure_wifi(config)
        elif self.access_point:
            bss_settings_2g: list[hostapd_bss_settings.BssSettings] = []
            bss_settings_5g: list[hostapd_bss_settings.BssSettings] = []
            bss_settings_2g.append(
                hostapd_bss_settings.BssSettings(
                    name=self.wpa2_network_2g.ssid,
                    ssid=self.wpa2_network_2g.ssid,
                    security=hostapd_security.Security(
                        security_mode=SecurityMode.WPA2,
                        password=self.wpa2_network_2g.password,
                    ),
                )
            )
            bss_settings_5g.append(
                hostapd_bss_settings.BssSettings(
                    name=self.wpa2_network_5g.ssid,
                    ssid=self.wpa2_network_5g.ssid,
                    security=hostapd_security.Security(
                        security_mode=SecurityMode.WPA2,
                        password=self.wpa2_network_5g.password,
                    ),
                )
            )
            ap_2g = hostapd_ap_preset.create_ap_preset(
                iface_wlan_2g=self.access_points[0].wlan_2g,
                iface_wlan_5g=self.access_points[0].wlan_5g,
                channel=hostapd_constants.AP_DEFAULT_CHANNEL_2G,
                ssid=self.open_network_2g.ssid,
                bss_settings=bss_settings_2g,
            )
            ap_5g = hostapd_ap_preset.create_ap_preset(
                iface_wlan_2g=self.access_points[0].wlan_2g,
                iface_wlan_5g=self.access_points[0].wlan_5g,
                channel=hostapd_constants.AP_DEFAULT_CHANNEL_5G,
                ssid=self.open_network_5g.ssid,
                bss_settings=bss_settings_5g,
            )
            # Start the networks
            self.access_point.start_ap(hostapd_config=ap_2g)
            self.access_point.start_ap(hostapd_config=ap_5g)

        # List of test SSIDs started by APs
        self.all_ssids: list[str] = [
            self.open_network_2g.ssid,
            self.wpa2_network_2g.ssid,
            self.open_network_5g.ssid,
            self.wpa2_network_5g.ssid,
        ]

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        await super().teardown_test()

    def _assert_network_is_in_results(
        self, scan_results: list[str], ssid: str
    ) -> None:
        """Verified scan results contain a specified network

        Args:
            scan_results: Scan results from a fuchsia Policy API scan.
            ssid: SSID for network that should be in the results.

        Raises:
            signals.TestFailure: if the network is not present in the scan results
        """
        asserts.assert_true(
            ssid in scan_results,
            f'Network "{ssid}" was not found in scan results: {scan_results}',
        )

    async def test_basic_scan_request(self) -> None:
        """Verify a scan returns all expected networks"""
        scan_results = await self.dut.wlan_policy.scan_for_networks()
        if len(scan_results) == 0:
            raise signals.TestFailure("Scan did not find any networks")
        for ssid in self.all_ssids:
            self._assert_network_is_in_results(scan_results, ssid)

    async def test_scan_while_connected_open_network_2g(self) -> None:
        """Connect to an open 2g network and perform a scan"""
        await self.dut.wlan_policy.save_network(
            self.open_network_2g.ssid,
            self.open_network_2g.security,
        )
        await self.dut.wlan_policy.connect(
            self.open_network_2g.ssid,
            self.open_network_2g.security,
        )

        scan_results = await self.dut.wlan_policy.scan_for_networks()
        for ssid in self.all_ssids:
            self._assert_network_is_in_results(scan_results, ssid)

    async def test_scan_while_connected_wpa2_network_2g(self) -> None:
        """Connect to a WPA2 2g network and perform a scan"""
        await self.dut.wlan_policy.save_network(
            self.wpa2_network_2g.ssid,
            self.wpa2_network_2g.security,
            self.wpa2_network_2g.password,
        )
        await self.dut.wlan_policy.connect(
            self.wpa2_network_2g.ssid,
            self.wpa2_network_2g.security,
        )

        scan_results = await self.dut.wlan_policy.scan_for_networks()
        for ssid in self.all_ssids:
            self._assert_network_is_in_results(scan_results, ssid)

    async def test_scan_while_connected_open_network_5g(self) -> None:
        """Connect to an open 5g network and perform a scan"""
        await self.dut.wlan_policy.save_network(
            self.open_network_5g.ssid,
            self.open_network_5g.security,
        )
        await self.dut.wlan_policy.connect(
            self.open_network_5g.ssid,
            self.open_network_5g.security,
        )

        scan_results = await self.dut.wlan_policy.scan_for_networks()
        for ssid in self.all_ssids:
            self._assert_network_is_in_results(scan_results, ssid)

    async def test_scan_while_connected_wpa2_network_5g(self) -> None:
        """Connect to a WPA2 5g network and perform a scan"""
        await self.dut.wlan_policy.save_network(
            self.wpa2_network_5g.ssid,
            self.wpa2_network_5g.security,
            self.wpa2_network_5g.password,
        )
        await self.dut.wlan_policy.connect(
            self.wpa2_network_5g.ssid,
            self.wpa2_network_5g.security,
        )

        scan_results = await self.dut.wlan_policy.scan_for_networks()
        for ssid in self.all_ssids:
            self._assert_network_is_in_results(scan_results, ssid)


if __name__ == "__main__":
    test_runner.main()
