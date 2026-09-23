# Copyright 2026 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

logger = logging.getLogger(__name__)
from datetime import datetime

import fidl_fuchsia_wlan_internal as f_wlan_internal
import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from antlion.controllers.access_point import AccessPoint, setup_ap
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.controllers.ap_lib.hostapd_security import (
    SecurityMode as DeprecatedSecurityMode,
)
from mobly import asserts, test_runner
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    SecurityOpen,
)

logger = logging.getLogger()


class WlanScanTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    phy: wlan_core.Phy

    async def setup_class(self) -> None:
        await super().setup_class()
        self.phy = await self.dut.wlan_core.ensure_single_phy()

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_core.destroy_all_ifaces()

    async def test_scan_while_connected(self) -> None:
        iface = await self.phy.create_client_iface()
        ssid = AccessPointConfig.random_string(20)
        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
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
            self.openwrt_ap.configure_wifi(config)
        elif isinstance(self.access_point, AccessPoint):
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=DEFAULT_2G_CHANNEL.number,
                ssid=ssid,
                security=DeprecatedSecurity(
                    security_mode=DeprecatedSecurityMode.OPEN,
                    password=None,
                ),
            )

        authentication = f_wlan_internal.Authentication(
            f_wlan_internal.Protocol.OPEN, None
        )

        name = self.dut.device_name

        logger.info('[%s] Scanning for ssid "%s"', name, ssid)
        scan_results = await iface.passive_scan()
        asserts.assert_in(
            ssid, scan_results, f'Scan results did not include "{ssid}"'
        )
        target_bss = scan_results[ssid]
        asserts.assert_equal(
            len(target_bss),
            1,
            f'Expected 1 BSS for "{ssid}", got {len(target_bss)}',
        )

        logger.info('[%s] Connecting to ssid "%s"', name, ssid)
        await iface.connect(
            ssid=ssid,
            bss_desc=target_bss[0],
            authentication=authentication,
        )

        logger.info('[%s] Scanning while connected to "%s"', name, ssid)
        start_time = datetime.now()
        scan_results = await iface.passive_scan()
        logger.info("Scan contained %d results", len(scan_results))
        logger.debug("Scan results: %s", scan_results)
        total_time_ms = (datetime.now() - start_time).total_seconds() * 1000
        logger.info(f"Scan time: {total_time_ms:.2f} ms")

        asserts.assert_in(
            ssid, scan_results, f'Scan results did not include "{ssid}"'
        )


if __name__ == "__main__":
    test_runner.main()
