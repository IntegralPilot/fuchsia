#!/usr/bin/env python3
#
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
from datetime import timedelta
from typing import Literal, NamedTuple, cast

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion import utils
from antlion.controllers.access_point import setup_ap
from antlion.controllers.ap_lib import hostapd_constants
from antlion.controllers.ap_lib.regulatory_channels import (
    COUNTRY_CHANNELS,
    TEST_CHANNELS,
)
from honeydew.affordances.connectivity.wlan.utils.types import CountryCode
from mobly import asserts, signals, test_runner
from mobly.config_parser import TestRunConfig
from openwrt_access_point.lib.access_point_config import (
    AccessPointConfig,
    Band,
    BssChannel,
    BssSettings,
    EhtMode,
    HeMode,
    RadioConfig,
    SecurityOpen,
)

N_CAPABILITIES_DEFAULT = [
    hostapd_constants.N_CAPABILITY_LDPC,
    hostapd_constants.N_CAPABILITY_SGI20,
    hostapd_constants.N_CAPABILITY_SGI40,
    hostapd_constants.N_CAPABILITY_TX_STBC,
    hostapd_constants.N_CAPABILITY_RX_STBC1,
]

MAX_2_4_CHANNEL = 14


class RegulatoryTest(NamedTuple):
    country_code: str
    channel: int
    channel_bandwidth: int
    expect_association: bool


class RegulatoryComplianceTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """Tests regulatory compliance.

    Testbed Requirement:
    * 1 x Fuchsia device (dut)
    * 1 x access point
    """

    def __init__(self, configs: TestRunConfig) -> None:
        super().__init__(configs)
        self.log = logging.getLogger()
        self.regulatory_results: list[str] = [
            "====CountryCode,Channel,Frequency,ChannelBandwith,Connected/Not-Connected===="
        ]

    async def setup_class(self) -> None:
        await super().setup_class()
        if not self.openwrt_ap and not self.access_point:
            raise signals.TestAbortClass("Requires at least one access point")

        if self.access_point:
            self.access_point.stop_all_aps()

    async def pre_run(self) -> None:
        tests: list[RegulatoryTest] = []
        for country in COUNTRY_CHANNELS.values():
            for channel, bandwidths in TEST_CHANNELS.items():
                for bandwidth in bandwidths:
                    tests.append(
                        RegulatoryTest(
                            country_code=country.country_code,
                            channel=channel,
                            channel_bandwidth=bandwidth,
                            expect_association=(
                                channel in country.allowed_channels
                                and bandwidth
                                in country.allowed_channels[channel]
                            ),
                        )
                    )

        def generate_test_name(
            country_code: str,
            channel: int,
            channel_bandwidth: int,
            _expect_association: bool,
        ) -> str:
            return (
                f"test_{country_code}_channel_{channel}_{channel_bandwidth}mhz"
            )

        self.generate_tests(
            self.verify_channel_compliance, generate_test_name, tests
        )

    async def teardown_class(self) -> None:
        regulatory_save_path = f"{self.log_path}/regulatory_results.txt"
        with open(regulatory_save_path, "w", encoding="utf-8") as file:
            file.write("\n".join(self.regulatory_results))

        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_class()

    async def setup_test(self) -> None:
        await super().setup_test()
        if self.access_point:
            self.access_point.stop_all_aps()
        await self.dut.wlan_policy.ensure_clean_state()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    def setup_ap(
        self,
        channel: int,
        channel_bandwidth: int,
    ) -> str:
        """Start network on AP with basic configuration.

        Args:
            channel: channel to use for network
            channel_bandwidth: channel bandwidth in mhz to use for network,

        Returns:
            SSID of the newly created and running network

        Raises:
            ConnectionError if network is not started successfully.
        """
        ssid = utils.rand_ascii_str(hostapd_constants.AP_SSID_LENGTH_2G)
        if self.openwrt_ap:
            band = Band.BAND_2G if channel <= MAX_2_4_CHANNEL else Band.BAND_5G
            bw_literal = cast(Literal[20, 40, 80, 160, 320], channel_bandwidth)

            if bw_literal == 320:
                phy_mode = EhtMode(bw=320)
            else:
                phy_mode = HeMode(bw=bw_literal)  # type: ignore

            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=BssChannel(
                            band=band, number=channel, phy_mode=phy_mode
                        ),
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=SecurityOpen(),
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
            return ssid
        elif self.access_point:
            try:
                setup_ap(
                    access_point=self.access_point,
                    profile_name="whirlwind",
                    channel=channel,
                    force_wmm=True,
                    ssid=ssid,
                    vht_bandwidth=channel_bandwidth,
                    setup_bridge=True,
                )
                self.log.info(
                    f"Network (ssid: {ssid}) up on channel {channel} "
                    f"w/ channel bandwidth {channel_bandwidth} MHz"
                )
                return ssid
            except Exception as err:
                raise ConnectionError(
                    f"Failed to setup ap on channel: {channel}, "
                    f"channel bandwidth: {channel_bandwidth} MHz. "
                ) from err
        else:
            raise ConnectionError("No access point available.")

    async def verify_channel_compliance(
        self,
        country_code: str,
        channel: int,
        channel_bandwidth: int,
        expect_association: bool,
    ) -> None:
        """Verify device complies with provided regulatory requirements for a
        specific channel and channel bandwidth. Run with generated test cases
        in the verify_regulatory_compliance parent test.
        """
        await self.dut.wlan_policy.set_country_code(CountryCode(country_code))

        ssid = self.setup_ap(channel, channel_bandwidth)

        self.log.info(
            f'Attempting to associate to network "{ssid}" on channel '
            f"{channel} @ {channel_bandwidth}mhz"
        )

        try:
            await self.dut.wlan_policy.save_network(
                ssid, f_wlan_policy.SecurityType.NONE
            )
            await self.dut.wlan_policy.connect(
                ssid,
                f_wlan_policy.SecurityType.NONE,
                timeout=timedelta(seconds=30),
            )
            associated = True
        except Exception as e:
            self.log.error(
                f"Failed to save and connect to {ssid} with error: {e}"
            )
            associated = False

        channel_ghz = "2.4" if channel < 36 else "5"
        association_code = "c" if associated else "nc"
        regulatory_result = f"REGTRACKER: {country_code},{channel},{channel_ghz},{channel_bandwidth},{association_code}"
        self.regulatory_results.append(regulatory_result)
        self.log.info(regulatory_result)

        asserts.assert_true(
            associated == expect_association,
            f"Expected device to{'' if expect_association else ' NOT'} "
            f"associate using country code {country_code} for channel "
            f"{channel} with channel bandwidth {channel_bandwidth} MHz.",
        )


if __name__ == "__main__":
    test_runner.main()
