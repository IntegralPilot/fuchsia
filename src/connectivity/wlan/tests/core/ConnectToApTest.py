# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fidl_fuchsia_wlan_internal as fidl_security
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
from mobly import signals, test_runner
from mobly.asserts import fail
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    Security,
    SecurityOpen,
    SecurityWpa2,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)

logger = logging.getLogger(__name__)


class ConnectToApTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
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
                (SecurityOpen(), None),
                (SecurityWpa2(), AccessPointConfig.random_string()),
            ],
        )

    def name_func(self, security: Security, password: str | None) -> str:
        return f"test_successfully_connect_to_ap_{security}"

    async def _test_logic(
        self, security: Security, password: str | None
    ) -> None:
        iface = await self.phy.create_client_iface()
        ssid = utils.rand_ascii_str(AP_SSID_LENGTH_2G)
        if self.openwrt_ap:
            self.openwrt_ap.configure_wifi(
                AccessPointConfig(
                    radios=[
                        RadioConfig(
                            channel=DEFAULT_2G_CHANNEL,
                            bss_settings=[
                                BssSettings(
                                    ssid=ssid,
                                    password=password,
                                    security=security,
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
                    security_mode=ConfigMapper.to_hostapd_security(security),
                    password=password,
                ),
            )

        protocol = fidl_security.Protocol.OPEN
        if isinstance(security, SecurityOpen):
            pass
        elif isinstance(security, SecurityWpa2):
            if password is None:
                raise signals.TestError("Password is required for WPA2")
            protocol = fidl_security.Protocol.WPA2_PERSONAL
        else:
            fail(f"Unsupported security mode: {security}")

        await iface.scan_and_connect(
            ssid=ssid, password=password, security=protocol
        )


if __name__ == "__main__":
    test_runner.main()
