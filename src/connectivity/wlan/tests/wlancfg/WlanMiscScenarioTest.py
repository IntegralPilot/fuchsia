#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion.controllers.access_point import setup_ap
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_5G_CHANNEL,
    AccessPointConfig,
    BssChannel,
    BssSettings,
    RadioConfig,
    Security,
    SecurityWpa2,
    SecurityWpa3,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)


class WlanMiscScenarioTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """Random scenario tests, usually to reproduce certain bugs, that do not
    fit into a specific test category, but should still be run in CI to catch
    regressions.
    """

    async def setup_class(self) -> None:
        await super().setup_class()
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )
        self.log = logging.getLogger()

        if not self.openwrt_aps and not self.access_points:
            raise signals.TestAbortClass("Requires at least one access point")

        if self.access_point:
            self.access_point.stop_all_aps()

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    def setup_ap(
        self,
        channel: BssChannel,
        ssid: str,
        security: Security,
        password: str | None = None,
    ) -> None:
        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=channel,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=security,
                                password=password,
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
        elif self.access_point:
            hostapd_band = ConfigMapper.to_hostapd_band(channel.band)
            hostapd_security = ConfigMapper.to_hostapd_security(security)
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=hostapd_band.default_channel(),
                ssid=ssid,
                security=DeprecatedSecurity(
                    security_mode=hostapd_security,
                    password=password,
                ),
            )

    async def test_connect_to_wpa2_after_wpa3_rejection(self) -> None:
        """Test association to non-WPA3 network after receiving a WPA3
        rejection, which was triggering a firmware hang.

        Bug: https://bugs.fuchsia.dev/p/fuchsia/issues/detail?id=71233
        """
        # Setup a WPA3 network
        wpa3_ssid = AccessPointConfig.random_string(8)
        wpa3_password = AccessPointConfig.random_string()
        self.setup_ap(
            channel=DEFAULT_5G_CHANNEL,
            ssid=wpa3_ssid,
            security=SecurityWpa3(),
            password=wpa3_password,
        )

        # Attempt to associate with wrong password, expecting failure
        self.log.info("Attempting to associate WPA3 with wrong password.")
        await self.dut.wlan_policy.save_network(
            wpa3_ssid,
            f_wlan_policy.SecurityType.WPA3,
            target_pwd="wrongpass",
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                wpa3_ssid,
                f_wlan_policy.SecurityType.WPA3,
            )

        if self.access_point:
            self.access_point.stop_all_aps()

        # Setup a WPA2 Network
        wpa2_ssid = AccessPointConfig.random_string(8)
        wpa2_password = AccessPointConfig.random_string()
        self.setup_ap(
            channel=DEFAULT_5G_CHANNEL,
            ssid=wpa2_ssid,
            security=SecurityWpa2(),
            password=wpa2_password,
        )

        # Attempt to associate, expecting success
        self.log.info("Attempting to associate with WPA2 network.")
        await self.dut.wlan_policy.save_network(
            wpa2_ssid,
            f_wlan_policy.SecurityType.WPA2,
            target_pwd=wpa2_password,
        )
        await self.dut.wlan_policy.connect(
            wpa2_ssid,
            f_wlan_policy.SecurityType.WPA2,
        )


if __name__ == "__main__":
    test_runner.main()
