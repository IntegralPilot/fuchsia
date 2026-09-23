# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fidl_fuchsia_wlan_common as fw_common
import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from honeydew.affordances.connectivity.wlan.utils.types import MacAddress
from mobly import asserts, test_runner

logger = logging.getLogger(__name__)


class CreateClientIfaceWithParticularMacTest(
    fuchsia_wlan_base_test.FuchsiaWlanBaseTest
):
    phy: wlan_core.Phy

    async def setup_class(self) -> None:
        await super().setup_class()
        self.phy = await self.dut.wlan_core.ensure_single_phy()

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_core.destroy_all_ifaces()

    async def test_create_client_iface_with_particular_mac(self) -> None:
        # Generate a valid randomized MAC to set in the driver
        random_sta_address = (
            MacAddress.random()
            .with_unicast_bit()
            .with_locally_administered_bit()
        )

        logger.info(f"Creating client iface with MAC {random_sta_address}")
        iface = await self.phy.create_client_iface(
            sta_address=random_sta_address
        )
        query_iface_response = await iface.query()
        asserts.assert_equal(iface.id, query_iface_response.id_)
        asserts.assert_equal(self.phy.id, query_iface_response.phy_id)
        asserts.assert_equal(
            fw_common.WlanMacRole.CLIENT, query_iface_response.role
        )
        asserts.assert_equal(
            random_sta_address,
            MacAddress(bytes(query_iface_response.sta_addr)),
        )


if __name__ == "__main__":
    test_runner.main()
