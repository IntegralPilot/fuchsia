# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import logging

import fidl_fuchsia_wlan_common as fidl_common
import fidl_fuchsia_wlan_device_service as fidl_device_svc
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
from mobly import signals, test_runner
from mobly.asserts import fail
from openwrt_access_point import AddrType as OpenWrtAddrType
from openwrt_access_point import InterfaceName as OpenWrtInterfaceName
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    SecurityOpen,
)

logger = logging.getLogger(__name__)


class FirmwarePowerModesTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
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
            arg_sets=[(mode,) for mode in fidl_common.PowerSaveType],
        )

    def name_func(self, ps_mode: fidl_common.PowerSaveType) -> str:
        return f"test_pm_mode_{ps_mode.name.replace('PS_MODE_', '').lower()}"

    async def _test_logic(self, ps_mode: fidl_common.PowerSaveType) -> None:
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
        else:
            raise signals.TestAbortClass(
                "No access point configured for this test."
            )

        ps_resp = await self.phy.device_monitor.set_power_save_mode(
            req=fidl_device_svc.SetPowerSaveModeRequest(
                phy_id=self.phy.id,
                ps_mode=ps_mode,
            )
        )
        assert (
            ps_resp.status == 0
        ), f"SetPowerSaveMode failed with status {ps_resp.status}"

        await iface.scan_and_connect(ssid=ssid)

        # TODO(http://b/371574733#comment6): Calling honeydew
        # methods results in a RuntimeError because an event loop already
        # exists. Otherwise, instead of sleeping, this test would call
        # these methods to check for an IP address:
        #    self.dut.update_wlan_interfaces()
        #    iface_name = self.dut.wlan_client_test_interface_name
        #    assert iface_name is not None, "Failed to get WLAN interface name"
        #    self.dut.wait_for_ipv4_addr(iface_name)
        # For now, wait for the DHCP server to assign the DUT an IP address.
        # This should take no more than 5 seconds, typically.
        await asyncio.sleep(10)

        if self.openwrt_ap:
            ap_address = self.openwrt_ap.get_addr(
                interface=OpenWrtInterfaceName.lan,
                addr_type=OpenWrtAddrType.ipv4_private,
            )
        elif isinstance(self.access_point, AccessPoint):
            ap_test_interface = self.access_point.wlan_2g
            ap_address = utils.get_addr(
                self.access_point.ssh, ap_test_interface
            )
        else:
            raise signals.TestAbortClass(
                "No access point configured for this test."
            )

        try:
            ping_result = await self.dut.netstack.ping(ap_address)
            logger.info(f"Ping succeeded: {ping_result.raw_output}")
        except Exception as e:
            logger.error(f"{e}")
            fail(f"Ping failed.")


if __name__ == "__main__":
    test_runner.main()
