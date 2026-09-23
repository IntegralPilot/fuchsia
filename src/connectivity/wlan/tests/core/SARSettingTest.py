# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fidl_fuchsia_wlan_internal as fidl_internal
import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from antlion import utils
from antlion.controllers.access_point import AccessPoint, setup_ap
from antlion.controllers.ap_lib.hostapd_constants import (
    AP_DEFAULT_CHANNEL_2G,
    AP_SSID_LENGTH_2G,
)
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.controllers.ap_lib.hostapd_security import (
    SecurityMode as DeprecatedSecurityMode,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    SecurityOpen,
)

logger = logging.getLogger(__name__)


class SARSettingTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    phy: wlan_core.Phy

    async def setup_class(self) -> None:
        await super().setup_class()
        self.phy = await self.dut.wlan_core.ensure_single_phy()

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_core.destroy_all_ifaces()

    async def pre_run(self) -> None:
        self.generate_tests(
            test_logic=self._test_logic,
            name_func=self.name_func,
            arg_sets=[
                # Trailing comma is load-bearing: it declares this as a singleton
                # tuple: https://docs.python.org/3/library/stdtypes.html#tuple
                (scenario,)
                for scenario in fidl_internal.TxPowerScenario
            ],
        )

    def name_func(self, scenario: fidl_internal.TxPowerScenario) -> str:
        return f"test_connect_with_sar_{scenario}"

    async def _test_logic(
        self, scenario: fidl_internal.TxPowerScenario
    ) -> None:
        iface = await self.phy.create_client_iface()
        # Setup AP
        ssid: str = utils.rand_ascii_str(AP_SSID_LENGTH_2G)
        if not self.openwrt_ap and not self.access_point:
            raise signals.TestAbortClass(
                "No access point configured for this test."
            )
        if self.openwrt_ap:
            self.openwrt_ap.configure_wifi(
                AccessPointConfig(
                    radios=[
                        RadioConfig(
                            channel=DEFAULT_2G_CHANNEL,
                            bss_settings=[
                                BssSettings(
                                    ssid=ssid,
                                    security=SecurityOpen(),
                                )
                            ],
                        )
                    ]
                )
            )
        elif isinstance(self.access_point, AccessPoint):
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=AP_DEFAULT_CHANNEL_2G,
                ssid=ssid,
                security=DeprecatedSecurity(
                    security_mode=DeprecatedSecurityMode.OPEN
                ),
            )

        # Set the SAR scenario
        (
            await self.phy.device_monitor.set_tx_power_scenario(
                phy_id=self.phy.id,
                scenario=scenario,
            )
        ).unwrap()

        # Connect to the AP
        await iface.scan_and_connect(ssid=ssid)

        # confirm the SAR scenario is still set
        get_sar_resp = (
            await self.phy.device_monitor.get_tx_power_scenario(
                phy_id=self.phy.id,
            )
        ).unwrap()
        asserts.assert_equal(
            get_sar_resp.scenario,
            scenario,
        )


if __name__ == "__main__":
    test_runner.main()
