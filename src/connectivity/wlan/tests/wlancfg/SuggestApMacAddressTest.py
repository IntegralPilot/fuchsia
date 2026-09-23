# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fidl_fuchsia_net as fidl_net
import fidl_fuchsia_wlan_policy as f_wlan_policy
import fidl_fuchsia_wlan_product_deprecatedconfiguration as fidl_deprecatedconfiguration
import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from honeydew.affordances.connectivity.wlan.utils.types import MacAddress
from honeydew.typing.custom_types import FidlEndpoint
from mobly import asserts, test_runner

logger = logging.getLogger(__name__)

TEST_SSID = "testssid"


class SuggestApMacAddressTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """Tests that we see the expected behavior with enabling and disabling
        client connections

    Test Bed Requirement:
    * One or more Fuchsia devices
    * One Access Point
    """

    phy: wlan_core.Phy

    async def setup_class(self) -> None:
        await super().setup_class()

        self.phy = await self.dut.wlan_core.ensure_single_phy()

        self.deprecated_configurator = fidl_deprecatedconfiguration.DeprecatedConfiguratorClient(
            self.dut.fuchsia_controller.connect_device_proxy(
                FidlEndpoint(
                    "core/wlancfg",
                    "fuchsia.wlan.product.deprecatedconfiguration.DeprecatedConfigurator",
                )
            )
        )

    async def test_suggest_ap_mac_address(self) -> None:
        """Tests suggest ap mac address through wlancfg

        1. Get initial mac address
        2. Suggest new mac address
        3. Verify new mac address is set successfully
        4. Reset to initial mac address
        5. Verify initial mac address is reset successfully
        """
        # Retrieve initial ap mac address
        logger.info("Creating SoftAP and retrieving its AP MAC address...")
        await self.dut.wlan_policy_ap.start(
            TEST_SSID,
            f_wlan_policy.SecurityType.NONE,
            None,
            f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
            f_wlan_policy.OperatingBand.ANY,
        )
        ap_ifaces = await self.phy.get_ap_ifaces()
        asserts.assert_equal(
            len(ap_ifaces),
            1,
            f"Expected exactly 1 AP interface on PHY, got {len(ap_ifaces)}",
        )
        initial_mac_addr = await ap_ifaces[0].get_mac_address()

        logger.info(f"Created SoftAP and retrieved MAC: {initial_mac_addr}")

        suggested_mac_addr = MacAddress(bytes([0x22 for _ in range(6)]))
        if initial_mac_addr == suggested_mac_addr:
            suggested_mac_addr = MacAddress(bytes([0x33 for _ in range(6)]))

        # Suggest and verify new mac address
        logger.info(
            f"Creating SoftAP with suggested MAC: {suggested_mac_addr}..."
        )
        await self.dut.wlan_policy_ap.stop_all()
        await self.deprecated_configurator.suggest_access_point_mac_address(
            mac=fidl_net.MacAddress(octets=bytes(suggested_mac_addr))
        )
        await self.dut.wlan_policy_ap.start(
            TEST_SSID,
            f_wlan_policy.SecurityType.NONE,
            None,
            f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
            f_wlan_policy.OperatingBand.ANY,
        )

        ap_ifaces = await self.phy.get_ap_ifaces()
        asserts.assert_equal(
            len(ap_ifaces),
            1,
            f"Expected exactly 1 AP interface on PHY, got {len(ap_ifaces)}",
        )
        set_mac_addr = await ap_ifaces[0].get_mac_address()
        if suggested_mac_addr != set_mac_addr:
            asserts.fail(
                f"Failed to set AP mac address via wlan_deprecated_configuration_lib. "
                f"Expected mac addr: {suggested_mac_addr}, Actual mac addr: {set_mac_addr}"
            )
        logger.info(
            f"Successfully created SoftAP with suggested MAC {suggested_mac_addr}."
        )

        # Reset to initial mac address and verify
        logger.info(f"Resetting to initial mac address ({initial_mac_addr}).")
        await self.dut.wlan_policy_ap.stop_all()
        await self.deprecated_configurator.suggest_access_point_mac_address(
            mac=fidl_net.MacAddress(octets=bytes(initial_mac_addr))
        )
        await self.dut.wlan_policy_ap.start(
            TEST_SSID,
            f_wlan_policy.SecurityType.NONE,
            None,
            f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
            f_wlan_policy.OperatingBand.ANY,
        )

        ap_ifaces = await self.phy.get_ap_ifaces()
        asserts.assert_equal(
            len(ap_ifaces),
            1,
            f"Expected exactly 1 AP interface on PHY, got {len(ap_ifaces)}",
        )
        set_mac_addr = await ap_ifaces[0].get_mac_address()
        if initial_mac_addr != set_mac_addr:
            asserts.fail(
                f"Failed to set AP mac address via wlan_deprecated_configuration_lib. "
                f"Expected mac addr: {initial_mac_addr}, Actual mac addr: {set_mac_addr}"
            )

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy_ap.stop_all()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy_ap.stop_all()
        await super().teardown_test()


if __name__ == "__main__":
    test_runner.main()
