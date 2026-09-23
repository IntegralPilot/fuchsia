# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

logger = logging.getLogger(__name__)

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from honeydew.affordances.connectivity.wlan.utils import errors as wlan_errors
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from mobly import asserts, signals, test_runner


class RegulatoryRecoveryTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    async def setup_class(self) -> None:
        await super().setup_class()

        await self.dut.wlan_policy.start_client_connections()
        self.device_supports_ap = True
        try:
            await self.dut.wlan_policy_ap.start(
                "test_ssid",
                f_wlan_policy.SecurityType.NONE,
                None,
                f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
                f_wlan_policy.OperatingBand.ANY,
            )
            await self.dut.wlan_policy_ap.stop_all()
        except wlan_errors.HoneydewWlanError:
            logger.info(
                "Detected this device does not support an access point interface."
            )
            self.device_supports_ap = False
        else:
            logger.info(
                "Detected this device supports an access point interface."
            )
            self.device_supports_ap = True

    async def test_interfaces_not_recreated_when_initially_disabled(
        self,
    ) -> None:
        """Test no interfaces created after applying a new country code."""

        # With the country code set to US, destroy all interfaces
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )
        await self.dut.wlan_policy.stop_client_connections(
            wait_for_confirmation=True
        )
        if self.device_supports_ap:
            await self.dut.wlan_policy_ap.stop_all()

        # Change the country code while all interfaces are destroyed
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["AUSTRALIA"]
        )

        # Verify changing the country code does not create interfaces
        await self.dut.wlan_policy.wait_for_client_state(
            f_wlan_policy.WlanClientState.CONNECTIONS_DISABLED
        )

        if self.device_supports_ap:
            await self.dut.wlan_policy_ap.set_new_update_listener()
            ap_updates = await self.dut.wlan_policy_ap.get_update()
            if ap_updates:
                raise signals.TestFailure(
                    f"AP in unexpected state: {ap_updates}"
                )

    async def test_interfaces_recreated_when_initially_enabled(self) -> None:
        """Test client and AP interfaces are automatically recreated after applying a new country code."""

        # With the country code set to US, create interfaces.
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )
        await self.dut.wlan_policy.start_client_connections()
        await self.dut.wlan_policy.wait_for_client_state(
            f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED
        )
        if self.device_supports_ap:
            await self.dut.wlan_policy_ap.start(
                "test_ssid",
                f_wlan_policy.SecurityType.NONE,
                None,
                f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
                f_wlan_policy.OperatingBand.ANY,
            )

        # Change the country code while interfaces are up.
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["AUSTRALIA"]
        )

        # Verify changing the country code cycles the client back to enabled.
        await self.dut.wlan_policy.wait_for_client_state(
            f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED
        )

        # Don't reset the update listener so that this verifies
        # changing the country code recreates the interfaces.
        if self.device_supports_ap:
            await self.dut.wlan_policy_ap.set_new_update_listener()
            ap_updates = await self.dut.wlan_policy_ap.get_update()
            if len(ap_updates) != 1:
                raise signals.TestFailure(f"No APs are running: {ap_updates}")
            if ap_updates[0].id_ is None:
                raise signals.TestFailure(
                    f"No network ID in AP update: {ap_updates}"
                )
            asserts.assert_equal(
                ap_updates[0].id_.ssid, "test_ssid", "Wrong ssid", ap_updates
            )
            asserts.assert_equal(
                ap_updates[0].id_.security_type,
                f_wlan_policy.SecurityType.NONE,
                "Wrong security type",
                ap_updates,
            )


if __name__ == "__main__":
    test_runner.main()
