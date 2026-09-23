# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from honeydew.typing.custom_types import MacAddress
from mobly import asserts, test_runner

logger = logging.getLogger(__name__)


class CreateClientIfaceWithInvalidMacTest(
    fuchsia_wlan_base_test.FuchsiaWlanBaseTest
):
    phy: wlan_core.Phy

    async def setup_class(self) -> None:
        await super().setup_class()
        self.phy = await self.dut.wlan_core.ensure_single_phy()

    async def test_create_client_iface_with_universal_unicast_mac(self) -> None:
        mac = (
            MacAddress.random()
            .with_universally_administered_bit()
            .with_unicast_bit()
        )

        logger.info(
            f"Attempting to create client iface with universally administered unicast MAC {mac}..."
        )

        try:
            await self.phy.create_client_iface(sta_address=mac)
        except AssertionError as e:
            logger.info(
                f"CreateIface correctly rejected the invalid MAC with error: {e}"
            )
            return

        asserts.fail(
            "CreateIface successfully created an interface with an invalid MAC address."
        )

    async def test_create_client_iface_with_local_multicast_mac(
        self,
    ) -> None:
        mac = (
            MacAddress.random()
            .with_locally_administered_bit()
            .with_multicast_bit()
        )

        logger.info(
            f"Attempting to create client iface with universally administered unicast MAC {mac}..."
        )

        try:
            await self.phy.create_client_iface(sta_address=mac)
        except AssertionError as e:
            logger.info(
                f"CreateIface correctly rejected the invalid MAC with error: {e}"
            )
            return

        asserts.fail(
            "CreateIface successfully created an interface with an invalid MAC address."
        )

    async def test_create_client_iface_with_universal_multicast_mac(
        self,
    ) -> None:
        mac = (
            MacAddress.random()
            .with_universally_administered_bit()
            .with_multicast_bit()
        )

        logger.info(
            f"Attempting to create client iface with universally administered unicast MAC {mac}..."
        )

        try:
            await self.phy.create_client_iface(sta_address=mac)
        except AssertionError as e:
            logger.info(
                f"CreateIface correctly rejected the invalid MAC with error: {e}"
            )
            return

        asserts.fail(
            "CreateIface successfully created an interface with an invalid MAC address."
        )


if __name__ == "__main__":
    test_runner.main()
