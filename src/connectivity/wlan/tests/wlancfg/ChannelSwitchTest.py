#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Tests STA handling of channel switch announcements.
"""

import asyncio
import logging
import random
import time
from typing import Sequence

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from antlion.controllers.access_point import setup_ap
from antlion.controllers.ap_lib import hostapd_constants
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point.lib.access_point_config import (
    DFS_BYPASS_COUNTRY_CODE,
    US_DFS_CHANNELS,
    AccessPointConfig,
    Band,
    BssChannel,
    BssSettings,
    HtMode,
    PhyMode,
    RadioConfig,
    SecurityOpen,
    VhtMode,
)
from openwrt_access_point.lib.uci_radio_options import UciRadioOptions

# Number of channel switch announcement beacons to send.
CSA_BEACON_COUNT = 10

# Beacon interval in unit of kus.
BEACON_INTERVAL_KUS = 100

# 1 kus = 1.024ms.
SEC_PER_KUS = 0.001024


class ChannelSwitchTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    phy: wlan_core.Phy
    client_iface: wlan_core.ClientIface

    # Time to wait between issuing channel switches
    WAIT_BETWEEN_CHANNEL_SWITCHES_S = 15

    # For operating class 115 tests.
    GLOBAL_OPERATING_CLASS_115_CHANNELS = [36, 40, 44, 48]
    # A channel outside the operating class.
    NON_GLOBAL_OPERATING_CLASS_115_CHANNEL = 52

    # For operating class 124 tests.
    GLOBAL_OPERATING_CLASS_124_CHANNELS = [149, 153, 157, 161]
    # A channel outside the operating class.
    NON_GLOBAL_OPERATING_CLASS_124_CHANNEL = 52

    async def setup_class(self) -> None:
        await super().setup_class()
        self.log = logging.getLogger()
        self.ssid = AccessPointConfig.random_string(10)

        # Set country code US for 5G DFS channels
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )

        self.phy = await self.dut.wlan_core.ensure_single_phy()

        if not self.openwrt_ap and not self.access_point:
            raise signals.TestAbortClass("Requires at least one access point")

        if self.access_point:
            self.access_point.stop_all_aps()

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()
        client_ifaces = await self.phy.get_client_ifaces()
        asserts.assert_equal(
            len(client_ifaces),
            1,
            f"Expected exactly 1 client interface on PHY, got {len(client_ifaces)}",
        )
        self.client_iface = client_ifaces[0]

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        if self.access_point:
            self.access_point.stop_all_aps()
        try:
            await self.dut.wlan_policy_ap.stop_all()
        except HoneydewWlanError as e:
            # This is expected for devices without soft-AP support.
            self.log.info("Failed to stop soft APs: %s", e)
        await super().teardown_test()

    async def channel_switch(
        self,
        band: Band,
        starting_channel: int,
        channel_switches: Sequence[int],
        test_with_soft_ap: bool = False,
    ) -> None:
        """Setup and run a channel switch test with the given parameters.

        Creates an AP, associates to it, and then issues channel switches
        through the provided channels. After each channel switch, the test
        checks that the DUT is connected for a period of time before considering
        the channel switch successful. If directed to start a SoftAP, the test
        will also check that the SoftAP is on the expected channel after each
        channel switch.

        Args:
            band: band that AP will use
            starting_channel: channel number that AP will use at startup
            channel_switches: ordered list of channels that the test will
                attempt to switch to
            test_with_soft_ap: whether to start a SoftAP before beginning the
                channel switches (default is False); note that if a SoftAP is
                started, the test will also check that the SoftAP handles
                channel switches correctly
        """
        current_channel = starting_channel

        phy_mode: PhyMode
        match band:
            case Band.BAND_2G:
                wlan_band = Band.BAND_2G
                phy_mode = HtMode(bw=20)
                if self.openwrt_ap:
                    ap_iface = self.openwrt_ap.wlan_2g_interface
                elif self.access_point:
                    ap_iface = self.access_point.wlan_2g
                else:
                    raise signals.TestAbortClass("No access point initialized")
            case Band.BAND_5G:
                wlan_band = Band.BAND_5G
                phy_mode = VhtMode(bw=20)
                if self.openwrt_ap:
                    ap_iface = self.openwrt_ap.wlan_5g_interface
                elif self.access_point:
                    ap_iface = self.access_point.wlan_5g
                else:
                    raise signals.TestAbortClass("No access point initialized")

        asserts.assert_true(
            self._channels_valid_for_band([current_channel], band),
            (
                f"starting channel {current_channel} not a valid channel for band {band}"
            ),
        )
        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig(
                        channel=BssChannel(
                            band=wlan_band,
                            number=current_channel,
                            phy_mode=phy_mode,
                        ),
                        custom_uci_options=UciRadioOptions(
                            beacon_int=BEACON_INTERVAL_KUS
                        ),
                        bss_settings=[
                            BssSettings(
                                ssid=self.ssid,
                                security=SecurityOpen(),
                            )
                        ],
                        country=DFS_BYPASS_COUNTRY_CODE,
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
        elif self.access_point:
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=current_channel,
                ssid=self.ssid,
                beacon_interval=BEACON_INTERVAL_KUS,
                # Antlion channel_switch currently only supports 20 MHz.
                vht_bandwidth=20,
            )

        if test_with_soft_ap:
            await self._start_soft_ap()
        self.log.info("connecting to network with ssid %s", self.ssid)
        await self.dut.wlan_policy.save_network(
            self.ssid, f_wlan_policy.SecurityType.NONE
        )
        await self.dut.wlan_policy.connect(
            self.ssid, f_wlan_policy.SecurityType.NONE
        )

        asserts.assert_true(
            channel_switches, "Cannot run test, no channels to switch to"
        )
        asserts.assert_true(
            self._channels_valid_for_band(channel_switches, band),
            (
                f"channel_switches {channel_switches} includes invalid channels "
                f"for band {band}"
            ),
        )

        for channel_num in channel_switches:
            if channel_num == current_channel:
                continue

            self.log.info(f"channel switch: {current_channel} -> {channel_num}")
            if self.openwrt_ap:
                if (
                    not self.openwrt_ap.allow_regdb_bypass
                    and channel_num in US_DFS_CHANNELS
                ):
                    self.log.info(f"Skipping DFS channel {channel_num}")
                    continue

                self.openwrt_ap.channel_switch(
                    ap_iface, channel_num, CSA_BEACON_COUNT
                )
                channel_num_after_switch = self.openwrt_ap.get_current_channel(
                    ap_iface
                )
            else:
                assert self.access_point is not None
                self.access_point.channel_switch(
                    ap_iface, channel_num, CSA_BEACON_COUNT
                )
                channel_num_after_switch = (
                    self.access_point.get_current_channel(ap_iface)
                )

            asserts.assert_equal(
                channel_num_after_switch,
                channel_num,
                "AP failed to channel switch",
            )
            previous_channel = current_channel
            current_channel = channel_num

            # Check periodically to see if DUT stays connected. Sometimes
            # CSA-induced disconnects occur seconds after last channel switch.

            change_channel_after = (
                time.time() + self.WAIT_BETWEEN_CHANNEL_SWITCHES_S
            )
            must_change_channel_within = (
                BEACON_INTERVAL_KUS * SEC_PER_KUS * CSA_BEACON_COUNT
            )
            must_change_channel_by = time.time() + must_change_channel_within

            while time.time() < change_channel_after:
                status = await self.client_iface.status()
                if status.connected is None:
                    raise signals.TestFailure(
                        f"want connected status, got {status} after "
                        f"switching from channel {previous_channel} to "
                        f"channel {current_channel}"
                    )

                got_channel = status.connected.primary.number

                if got_channel == previous_channel:
                    if time.time() > must_change_channel_by:
                        raise signals.TestFailure(
                            f"Failed to switch channel: expected {current_channel}, "
                            f"but remained on {got_channel} "
                            f"after the {must_change_channel_within:.2f}s timeout expired."
                        )
                    await asyncio.sleep(0.1)
                    continue

                asserts.assert_equal(
                    got_channel,
                    current_channel,
                    f"want channel={current_channel}, got {got_channel}",
                )
                if test_with_soft_ap:
                    soft_ap_channel = await self._soft_ap_channel()
                    asserts.assert_equal(
                        soft_ap_channel,
                        channel_num,
                        f"SoftAP interface on wrong channel ({soft_ap_channel})",
                    )
                await asyncio.sleep(1)

    async def test_channel_switch_2g(self) -> None:
        """Channel switch through all (US only) channels in the 2 GHz band."""
        await self.channel_switch(
            band=Band.BAND_2G,
            starting_channel=hostapd_constants.AP_DEFAULT_CHANNEL_2G,
            channel_switches=hostapd_constants.US_CHANNELS_2G,
        )

    async def test_channel_switch_2g_with_soft_ap(self) -> None:
        """Channel switch through (US only) 2 Ghz channels with SoftAP up."""
        await self.channel_switch(
            band=Band.BAND_2G,
            starting_channel=hostapd_constants.AP_DEFAULT_CHANNEL_2G,
            channel_switches=hostapd_constants.US_CHANNELS_2G,
            test_with_soft_ap=True,
        )

    async def test_channel_switch_2g_shuffled_with_soft_ap(self) -> None:
        """Switch through shuffled (US only) 2 Ghz channels with SoftAP up."""
        channels = hostapd_constants.US_CHANNELS_2G
        random.shuffle(channels)
        self.log.info(f"Shuffled channel switch sequence: {channels}")
        await self.channel_switch(
            band=Band.BAND_2G,
            starting_channel=hostapd_constants.AP_DEFAULT_CHANNEL_2G,
            channel_switches=channels,
            test_with_soft_ap=True,
        )

    async def test_channel_switch_5g(self) -> None:
        """Channel switch through all (US only) channels in the 5 GHz band."""
        await self.channel_switch(
            band=Band.BAND_5G,
            starting_channel=hostapd_constants.AP_DEFAULT_CHANNEL_5G,
            channel_switches=hostapd_constants.US_CHANNELS_5G,
        )

    async def test_channel_switch_5g_with_soft_ap(self) -> None:
        """Channel switch through (US only) 5 GHz channels with SoftAP up."""
        await self.channel_switch(
            band=Band.BAND_5G,
            starting_channel=hostapd_constants.AP_DEFAULT_CHANNEL_5G,
            channel_switches=hostapd_constants.US_CHANNELS_5G,
            test_with_soft_ap=True,
        )

    async def test_channel_switch_5g_shuffled_with_soft_ap(self) -> None:
        """Switch through shuffled (US only) 5 Ghz channels with SoftAP up."""
        channels = hostapd_constants.US_CHANNELS_5G
        random.shuffle(channels)
        self.log.info(f"Shuffled channel switch sequence: {channels}")
        await self.channel_switch(
            band=Band.BAND_5G,
            starting_channel=hostapd_constants.AP_DEFAULT_CHANNEL_5G,
            channel_switches=channels,
            test_with_soft_ap=True,
        )

    async def test_channel_switch_regression_global_operating_class_115(
        self,
    ) -> None:
        """Channel switch into, through, and out of global op. class 115 channels.

        Global operating class 115 is described in IEEE 802.11-2016 Table E-4.
        Regression test for fxbug.dev/42165602.
        """
        channels = self.GLOBAL_OPERATING_CLASS_115_CHANNELS + [
            self.NON_GLOBAL_OPERATING_CLASS_115_CHANNEL
        ]
        await self.channel_switch(
            band=Band.BAND_5G,
            starting_channel=self.NON_GLOBAL_OPERATING_CLASS_115_CHANNEL,
            channel_switches=channels,
        )

    async def test_channel_switch_regression_global_operating_class_115_with_soft_ap(
        self,
    ) -> None:
        """Test global operating class 124 channel switches, with SoftAP.

        Regression test for fxbug.dev/42165602.
        """
        channels = self.GLOBAL_OPERATING_CLASS_115_CHANNELS + [
            self.NON_GLOBAL_OPERATING_CLASS_115_CHANNEL
        ]
        await self.channel_switch(
            band=Band.BAND_5G,
            starting_channel=self.NON_GLOBAL_OPERATING_CLASS_115_CHANNEL,
            channel_switches=channels,
            test_with_soft_ap=True,
        )

    async def test_channel_switch_regression_global_operating_class_124(
        self,
    ) -> None:
        """Switch into, through, and out of global op. class 124 channels.

        Global operating class 124 is described in IEEE 802.11-2016 Table E-4.
        Regression test for fxbug.dev/42142868.
        """
        channels = self.GLOBAL_OPERATING_CLASS_124_CHANNELS + [
            self.NON_GLOBAL_OPERATING_CLASS_124_CHANNEL
        ]
        await self.channel_switch(
            band=Band.BAND_5G,
            starting_channel=self.NON_GLOBAL_OPERATING_CLASS_124_CHANNEL,
            channel_switches=channels,
        )

    async def test_channel_switch_regression_global_operating_class_124_with_soft_ap(
        self,
    ) -> None:
        """Test global operating class 124 channel switches, with SoftAP.

        Regression test for fxbug.dev/42142868.
        """
        channels = self.GLOBAL_OPERATING_CLASS_124_CHANNELS + [
            self.NON_GLOBAL_OPERATING_CLASS_124_CHANNEL
        ]
        await self.channel_switch(
            band=Band.BAND_5G,
            starting_channel=self.NON_GLOBAL_OPERATING_CLASS_124_CHANNEL,
            channel_switches=channels,
            test_with_soft_ap=True,
        )

    def _channels_valid_for_band(
        self, channels: Sequence[int], band: Band
    ) -> bool:
        """Determine if the channels are valid for the band (US only).

        Args:
            channels: channel numbers
            band: a valid band
        """
        channels_set = frozenset(channels)
        match band:
            case Band.BAND_2G:
                band_channels = frozenset(hostapd_constants.US_CHANNELS_2G)
            case Band.BAND_5G:
                band_channels = frozenset(hostapd_constants.US_CHANNELS_5G)
        return channels_set <= band_channels

    async def _start_soft_ap(self) -> None:
        """Start a SoftAP on the DUT.

        Raises:
            EnvironmentError: if the SoftAP does not start
        """
        ssid = AccessPointConfig.random_string(10)
        self.log.info(f'Starting SoftAP on DUT with ssid "{ssid}"')

        await self.dut.wlan_policy_ap.start(
            ssid,
            f_wlan_policy.SecurityType.NONE,
            None,
            f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
            f_wlan_policy.OperatingBand.ANY,
        )
        self.log.info(f"SoftAp network ({ssid}) is up.")

    async def _soft_ap_channel(self) -> int:
        """Determine the channel of the DUT SoftAP interface.

        If the interface is not connected, the method will assert a test
        failure.

        Returns: channel number

        Raises:
            EnvironmentError: if SoftAP interface channel cannot be determined.
            signals.TestFailure: when the SoftAP interface is not connected.
        """
        ap_ifaces = await self.phy.get_ap_ifaces()
        asserts.assert_equal(
            len(ap_ifaces),
            1,
            f"Expected exactly 1 AP interface on PHY, got {len(ap_ifaces)}",
        )
        status = await self.client_iface.status()
        if status.connected is None:
            raise signals.TestFailure(f"want connected status, got {status}")
        return status.connected.primary.number


if __name__ == "__main__":
    test_runner.main()
