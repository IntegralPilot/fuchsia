#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
import time
from datetime import timedelta

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion.controllers.access_point import setup_ap
from antlion.controllers.ap_lib import hostapd_constants
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.controllers.ap_lib.hostapd_security import (
    SecurityMode as DeprecatedSecurityMode,
)
from antlion.utils import rand_ascii_str
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from mobly import signals, test_runner
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    SecurityWpa2,
)

# These tests should have a longer timeout for connecting than normal connect
# tests because the device should probabilistically perform active scans for
# hidden networks. Multiple scans are necessary to verify a very low chance of
# random failure.
TIME_WAIT_FOR_CONNECT = timedelta(seconds=90)
TIME_ATTEMPT_SCANS = timedelta(seconds=90)


class HiddenNetworksTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """Tests for Hidden Networks.

    Test Bed Requirement:
    * One or more Fuchsia devices
    * One Access Point
    """

    async def setup_class(self) -> None:
        await super().setup_class()
        self.log = logging.getLogger()

        if not self.openwrt_aps and not self.access_points:
            raise signals.TestAbortClass("Requires at least one access point.")

        # Start an AP with a hidden network
        # TODO(https://fxbug.dev/489256041): Delete references to old AP when OpenWRT migration
        # is complete.
        if self.openwrt_aps:
            self.openwrt_ap = self.openwrt_aps[0]
            self.hidden_ssid = AccessPointConfig.random_string()
            self.hidden_password = AccessPointConfig.random_string()
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_2G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=self.hidden_ssid,
                                security=SecurityWpa2(),
                                password=self.hidden_password,
                                hidden=True,
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)

        elif self.access_points:
            self.hidden_ssid = rand_ascii_str(
                hostapd_constants.AP_SSID_LENGTH_2G
            )
            self.hidden_password = rand_ascii_str(
                hostapd_constants.AP_PASSPHRASE_LENGTH_2G
            )
            self.access_point = self.access_points[0]
            self.access_point.stop_all_aps()
            setup_ap(
                self.access_point,
                "whirlwind",
                hostapd_constants.AP_DEFAULT_CHANNEL_2G,
                self.hidden_ssid,
                hidden=True,
                security=DeprecatedSecurity(
                    security_mode=DeprecatedSecurityMode.WPA2,
                    password=self.hidden_password,
                ),
            )

        if len(self.fuchsia_devices) < 1:
            raise EnvironmentError("No Fuchsia devices found.")

    async def teardown_class(self) -> None:
        # We don't need this cleanup for OpenWRT APs, as the controller handles it in its destroy method.
        if hasattr(self, "access_point") and self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_class()

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        await super().teardown_test()

    # Tests

    async def test_scan_hidden_networks(self) -> None:
        """Probabilistic test to see if we can see hidden networks with a scan.

        Scan a few times and check that we see the hidden networks in the results at
        least once. We stop client connections to not trigger a connect when saving,
        which would interfere with requested scans.

        Raises:
            TestFailure if we fail to see hidden network in scans before timing out.
        """
        await self.dut.wlan_policy.stop_client_connections(
            wait_for_confirmation=True
        )
        await self.dut.wlan_policy.save_network(
            self.hidden_ssid,
            f_wlan_policy.SecurityType.WPA2,
            self.hidden_password,
        )
        await self.dut.wlan_policy.start_client_connections()
        start_time = time.time()
        num_performed_scans = 0

        while time.time() < start_time + TIME_ATTEMPT_SCANS.total_seconds():
            num_performed_scans = num_performed_scans + 1
            scan_result = await self.dut.wlan_policy.scan_for_networks()

            if self.hidden_ssid in scan_result:
                self.log.info(
                    f"SSID of hidden network seen after {num_performed_scans} scans"
                )
                return
            time.sleep(1)

        self.log.error(f"Failed to see SSID after {num_performed_scans} scans")
        raise signals.TestFailure("Failed to see hidden network in scans")

    async def test_auto_connect_hidden_on_startup(self) -> None:
        """Test auto connect on startup.

        This test checks that if we are not connected to anything but have a hidden
        network saved, we will eventually actively scan for it and connect.

        Raises:
            TestFailure if the client fails to auto connect to the hidden network.
        """
        # Start up AP with an open network with a random SSID

        await self.dut.wlan_policy.stop_client_connections(
            wait_for_confirmation=True
        )
        await self.dut.wlan_policy.save_network(
            self.hidden_ssid,
            f_wlan_policy.SecurityType.WPA2,
            self.hidden_password,
        )

        # Reboot the device and check that it auto connects.
        await self.dut.reboot()
        await self.dut.wlan_policy.start_client_connections()
        try:
            await self.dut.wlan_policy.wait_for_network_state(
                self.hidden_ssid,
                f_wlan_policy.ConnectionState.CONNECTED,
                timeout=TIME_WAIT_FOR_CONNECT,
            )
        except HoneydewWlanError as e:
            raise signals.TestFailure(
                "Failed to auto connect to hidden network on startup"
            ) from e

    async def test_auto_connect_hidden_on_save(self) -> None:
        """Test auto connect to hidden network on save.

        This test checks that if we save a hidden network and are not connected to
        anything, the device will connect to the hidden network that was just saved.

        Raises:
            TestFailure if client fails to auto connect to a hidden network after saving
            it.
        """
        await self.dut.wlan_policy.wait_for_no_connections()
        await self.dut.wlan_policy.save_network(
            self.hidden_ssid,
            f_wlan_policy.SecurityType.WPA2,
            self.hidden_password,
        )
        try:
            await self.dut.wlan_policy.wait_for_network_state(
                self.hidden_ssid,
                f_wlan_policy.ConnectionState.CONNECTED,
                timeout=TIME_WAIT_FOR_CONNECT,
            )
        except HoneydewWlanError as e:
            raise signals.TestFailure(
                "Failed to auto connect to hidden network on save"
            ) from e


if __name__ == "__main__":
    test_runner.main()
