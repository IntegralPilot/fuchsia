#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Script for testing WiFi recovery after rebooting the AP.

Override default number of iterations using the following
parameter in the test config file.

"beacon_loss_test_iterations": "5"
"""

import asyncio
import logging

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion.controllers.access_point import setup_ap
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from mobly import signals, test_runner
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    DEFAULT_5G_CHANNEL,
    AccessPointConfig,
    Band,
    BssChannel,
    BssSettings,
    RadioConfig,
    SecurityOpen,
)


class BeaconLossTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    # Default number of test iterations here.
    # Override using parameter in config file.
    # Eg: "beacon_loss_test_iterations": "10"
    num_of_iterations = 5

    # Time to wait for AP to startup
    wait_ap_startup_s = 15

    # Default wait time in seconds for the AP radio to turn back on
    wait_to_connect_after_ap_txon_s = 5

    async def setup_class(self) -> None:
        await super().setup_class()
        self.log = logging.getLogger()
        self.ssid = AccessPointConfig.random_string(10)

        if not self.openwrt_ap and not self.access_point:
            raise signals.TestAbortClass("Requires at least one access point")

        if self.access_point:
            self.access_point.stop_all_aps()

        self.num_of_iterations = int(
            self.user_params.get(
                "beacon_loss_test_iterations", self.num_of_iterations
            )
        )
        self.in_use_interface: str | None = None

        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        # ensure radio is on, in case the test failed while the radio was off
        if self.in_use_interface:
            if self.openwrt_ap:
                self.openwrt_ap.reset_txpower(self.in_use_interface)
            elif self.access_point:
                self.access_point.iwconfig.ap_iwconfig(
                    self.in_use_interface, "txpower on"
                )
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    async def beacon_loss(self, channel: BssChannel) -> None:
        band = channel.band

        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=channel,
                        bss_settings=[
                            BssSettings(
                                ssid=self.ssid,
                                security=SecurityOpen(),
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
            self.in_use_interface = (
                self.openwrt_ap.wlan_5g_interface
                if band == Band.BAND_5G
                else self.openwrt_ap.wlan_2g_interface
            )
        elif self.access_point:
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=channel.number,
                ssid=self.ssid,
            )
            if channel.number > 14:
                self.in_use_interface = self.access_point.wlan_5g
            else:
                self.in_use_interface = self.access_point.wlan_2g

        assert self.in_use_interface is not None
        await asyncio.sleep(self.wait_ap_startup_s)
        self.log.info(
            f"Initial association with SSID: {self.ssid} on channel {channel}"
        )
        await self.dut.wlan_policy.save_network(
            self.ssid, f_wlan_policy.SecurityType.NONE
        )
        await self.dut.wlan_policy.connect(
            self.ssid, f_wlan_policy.SecurityType.NONE
        )
        self.log.info(
            f"Successfully associated and connected to SSID: {self.ssid}"
        )

        for i in range(self.num_of_iterations):
            self.log.info(
                f"Iteration {i + 1}/{self.num_of_iterations}: Testing beacon loss on interface {self.in_use_interface}"
            )
            # Turn off AP radio
            self.log.info(
                f"Turning off AP radio for interface: {self.in_use_interface}"
            )
            if self.openwrt_ap:
                self.openwrt_ap.set_txpower(self.in_use_interface, 0)
            elif self.access_point:
                self.access_point.iwconfig.ap_iwconfig(
                    self.in_use_interface, "txpower off"
                )
            await self.dut.wlan_policy.wait_for_no_connections()
            self.log.info(f"DUT successfully disconnected from {self.ssid}")

            # Turn on AP radio
            self.log.info(
                f"Turning on AP radio for interface: {self.in_use_interface}"
            )
            assert self.in_use_interface is not None
            if self.openwrt_ap:
                self.openwrt_ap.reset_txpower(self.in_use_interface)
            elif self.access_point:
                self.access_point.iwconfig.ap_iwconfig(
                    self.in_use_interface, "txpower on"
                )
            await asyncio.sleep(self.wait_to_connect_after_ap_txon_s)

            # Initiate reconnection
            self.log.info(f"Sending associate command for SSID {self.ssid}")
            await self.dut.wlan_policy.connect(
                self.ssid, f_wlan_policy.SecurityType.NONE
            )
            self.log.info(f"DUT successfully reconnected to {self.ssid}")

    async def test_beacon_loss_2g(self) -> None:
        await self.beacon_loss(channel=DEFAULT_2G_CHANNEL)

    async def test_beacon_loss_5g(self) -> None:
        await self.beacon_loss(channel=DEFAULT_5G_CHANNEL)


if __name__ == "__main__":
    test_runner.main()
