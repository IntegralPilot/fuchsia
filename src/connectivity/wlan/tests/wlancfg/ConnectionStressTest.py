#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Script for testing WiFi connection and disconnection in a loop
"""

import logging
import time
from dataclasses import dataclass
from datetime import timedelta

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion.controllers.access_point import setup_ap
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    DEFAULT_5G_CHANNEL,
    AccessPointConfig,
    BssChannel,
    BssSettings,
    RadioConfig,
    Security,
    SecurityOpen,
    SecurityWpa2,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)


@dataclass
class TestParams:
    profile: str
    channel: BssChannel
    security: Security
    ap_ssid: str
    ap_password: str | None
    dut_ssid: str
    dut_password: str | None
    expect_associated: bool


class ConnectionStressTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    # Default number of test iterations here.
    # Override using parameter in config file.
    # Eg: "connection_stress_test_iterations": "50"
    num_of_iterations = 10

    async def pre_run(self) -> None:
        tests: list[TestParams] = []

        # Successful associate
        for profile in [
            "whirlwind",
            "whirlwind_11ab_legacy",
            "whirlwind_11ag_legacy",
        ]:
            for channel in [DEFAULT_2G_CHANNEL, DEFAULT_5G_CHANNEL]:
                ssid = AccessPointConfig.random_string(10)
                tests.append(
                    TestParams(
                        profile=profile,
                        channel=channel,
                        security=SecurityOpen(),
                        ap_ssid=ssid,
                        ap_password=None,
                        dut_ssid=ssid,
                        dut_password=None,
                        expect_associated=True,
                    )
                )

        # Wrong SSID
        for channel in [DEFAULT_2G_CHANNEL, DEFAULT_5G_CHANNEL]:
            ssid = AccessPointConfig.random_string(10)
            tests.append(
                TestParams(
                    profile="whirlwind",
                    channel=channel,
                    security=SecurityOpen(),
                    ap_ssid=ssid,
                    ap_password=None,
                    dut_ssid=f"wrong_{ssid}",
                    dut_password=None,
                    expect_associated=False,
                )
            )

        # Wrong password
        for channel in [DEFAULT_2G_CHANNEL, DEFAULT_5G_CHANNEL]:
            ssid = AccessPointConfig.random_string(10)
            password = AccessPointConfig.random_string(20)
            tests.append(
                TestParams(
                    profile="whirlwind",
                    channel=channel,
                    security=SecurityWpa2(),
                    ap_ssid=ssid,
                    ap_password=password,
                    dut_ssid=ssid,
                    dut_password=f"wrong_{password}",
                    expect_associated=False,
                )
            )

        def test_name(test: TestParams) -> str:
            band = test.channel.band.lower()
            if test.expect_associated:
                return f"test_{test.profile}_{band}"
            if test.ap_ssid != test.dut_ssid:
                return f"test_{test.profile}_{band}_wrong_ssid"
            if test.ap_password != test.dut_password:
                return f"test_{test.profile}_{band}_wrong_password"
            raise TypeError(f"Unknown name for {test}")

        self.generate_tests(
            self.connect_disconnect, test_name, [(t,) for t in tests]
        )

    async def setup_class(self) -> None:
        await super().setup_class()
        self.log = logging.getLogger()
        self.ssid = AccessPointConfig.random_string(10)

        # Set country code US for 5G DFS channels
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )

        # The base class FuchsiaWlanBaseTest setup_class registers access_points
        # and openwrt_aps automatically.
        if not self.openwrt_aps and not self.access_points:
            raise signals.TestAbortClass("Requires at least one access point")

        # Since the base class setup_class does NOT call stop_all_aps on access_point,
        # let's preserve that setup behavior here:
        if self.access_point:
            self.access_point.stop_all_aps()

        self.num_of_iterations = int(
            self.user_params.get(
                "connection_stress_test_iterations", self.num_of_iterations
            )
        )
        self.log.info(f"iterations: {self.num_of_iterations}")

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        if self.openwrt_ap:
            self.openwrt_ap.stop_wifi()
        elif self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    async def teardown_class(self) -> None:
        # We don't need this cleanup for OpenWRT APs, as the controller handles it in its destroy method.
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_class()

    async def connect_disconnect(self, test: TestParams) -> None:
        """Helper to start an AP, connect DUT to it and disconnect

        Args:
            test: TestParams containing configuration
        """
        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=test.channel,
                        bss_settings=[
                            BssSettings(
                                ssid=test.ap_ssid,
                                security=test.security,
                                password=test.ap_password,
                            ),
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
        elif self.access_point:
            security = ConfigMapper.to_hostapd_security(test.security)
            setup_ap(
                access_point=self.access_point,
                profile_name=test.profile,
                channel=test.channel.number,
                ssid=test.ap_ssid,
                security=DeprecatedSecurity(
                    security_mode=security, password=test.ap_password
                ),
            )

        if isinstance(test.security, SecurityOpen):
            security_type = f_wlan_policy.SecurityType.NONE
        elif isinstance(test.security, SecurityWpa2):
            security_type = f_wlan_policy.SecurityType.WPA2
        else:
            raise TypeError(f"Unsupported security type: {test.security}")

        for iteration in range(0, self.num_of_iterations):
            associated = False
            try:
                await self.dut.wlan_policy.save_network(
                    test.dut_ssid,
                    security_type,
                    target_pwd=test.dut_password,
                )
                await self.dut.wlan_policy.connect(
                    test.dut_ssid,
                    security_type,
                    timeout=timedelta(seconds=30),
                )
                associated = True
            except Exception as e:
                self.log.info(f"Connection failed on attempt {iteration}: {e}")

            asserts.assert_equal(
                associated,
                test.expect_associated,
                (
                    f"Attempt {iteration}/{self.num_of_iterations}: "
                    f"associated={associated}, want {test.expect_associated}"
                ),
            )

            await self.dut.wlan_policy.remove_all_networks()
            await self.dut.wlan_policy.wait_for_no_connections()

            # Wait a second before trying again
            time.sleep(1)


if __name__ == "__main__":
    test_runner.main()
