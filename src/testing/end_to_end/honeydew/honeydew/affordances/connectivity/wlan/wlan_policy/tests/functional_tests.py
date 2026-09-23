# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for wlan policy affordance."""

import time
from typing import AsyncIterator

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion.controllers import access_point
from antlion.controllers.ap_lib import hostapd_constants
from honeydew.affordances.connectivity.netstack.types import PortClass
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    ClientStateSummary,
    NetworkConfig,
    NetworkIdentifier,
    NetworkState,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    SecurityOpen,
)

# Time to wait for a WLAN interface to become available.
WLAN_INTERFACE_TIMEOUT = 30
# Time to wait for a WLAN client state update.
DEFAULT_GET_UPDATE_TIMEOUT = 60


class WlanPolicyTests(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """WlanPolicy affordance tests"""

    async def setup_class(self) -> None:
        """setup_class is called once before running tests."""
        await super().setup_class()

        await self.wait_for_interface(self.dut.netstack, PortClass.WLAN_CLIENT)

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.remove_all_networks()

    async def teardown_test(self) -> None:
        if self.access_point is not None:
            self.access_point.close()
        await super().teardown_test()

    async def test_client_methods(self) -> None:
        """Test case for wlan_policy client methods.

        This test starts and stops client connections and checks that they are in the
        expected states.
        """
        await self.dut.wlan_policy.start_client_connections()
        await self.dut.wlan_policy.set_new_update_listener()
        asserts.assert_equal(
            await self.dut.wlan_policy.get_update(),
            ClientStateSummary(
                state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
                networks=[],
            ),
        )

        await self.dut.wlan_policy.stop_client_connections(
            wait_for_confirmation=False
        )
        asserts.assert_equal(
            await self.dut.wlan_policy.get_update(),
            ClientStateSummary(
                state=f_wlan_policy.WlanClientState.CONNECTIONS_DISABLED,
                networks=[],
            ),
        )

        # Verify connections are still disabled after resetting the update
        # listener.
        await self.dut.wlan_policy.set_new_update_listener()
        asserts.assert_equal(
            await self.dut.wlan_policy.get_update(),
            ClientStateSummary(
                state=f_wlan_policy.WlanClientState.CONNECTIONS_DISABLED,
                networks=[],
            ),
        )

    async def test_ap_auto_connect(self) -> None:
        """Verify Fuchsia can auto-connect to a saved network."""
        if not self.openwrt_ap and not self.access_point:
            raise signals.TestSkip("Access point required for this test")

        test_ssid = AccessPointConfig.random_string()
        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_2G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=test_ssid,
                                security=SecurityOpen(),
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
        else:
            assert self.access_point is not None
            access_point.setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=hostapd_constants.AP_DEFAULT_CHANNEL_2G,
                ssid=test_ssid,
            )

        await self.dut.wlan_policy.start_client_connections()
        await self.dut.wlan_policy.set_new_update_listener()
        asserts.assert_equal(
            await self.dut.wlan_policy.get_update(),
            ClientStateSummary(
                state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
                networks=[],
            ),
        )

        # Verify the access point came up
        asserts.assert_in(
            test_ssid,
            await self.dut.wlan_policy.scan_for_networks(),
            f'ssid "{test_ssid}" not found in scan results; check connection to the AP',
        )

        # Saving the network should initiate an auto-connection.
        await self.dut.wlan_policy.save_network(
            test_ssid, f_wlan_policy.SecurityType.NONE
        )
        asserts.assert_equal(
            await self.dut.wlan_policy.get_saved_networks(),
            [
                NetworkConfig(
                    test_ssid, f_wlan_policy.SecurityType.NONE, "None", ""
                )
            ],
        )
        await self.wait_for_network(
            test_ssid, f_wlan_policy.ConnectionState.CONNECTING
        )
        await self.wait_for_network(
            test_ssid, f_wlan_policy.ConnectionState.CONNECTED
        )

        # Connecting explicitly again shouldn't do anything.
        await self.dut.wlan_policy.connect(
            test_ssid, f_wlan_policy.SecurityType.NONE
        )
        async for update in self.get_updates_until(timeout_sec=3):
            asserts.fail(f"Expected no updates, got {update}")

        # Stopping client connections should initiate a auto-disconnection.
        await self.dut.wlan_policy.stop_client_connections(
            wait_for_confirmation=False
        )
        asserts.assert_equal(
            await self.dut.wlan_policy.get_update(),
            ClientStateSummary(
                state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
                networks=[
                    NetworkState(
                        NetworkIdentifier(
                            test_ssid, f_wlan_policy.SecurityType.NONE
                        ),
                        f_wlan_policy.ConnectionState.DISCONNECTED,
                        f_wlan_policy.DisconnectStatus.CONNECTION_STOPPED,
                    )
                ],
            ),
        )
        asserts.assert_equal(
            await self.dut.wlan_policy.get_update(),
            ClientStateSummary(
                state=f_wlan_policy.WlanClientState.CONNECTIONS_DISABLED,
                networks=[],
            ),
        )

        # Starting client connections again should initiate an auto-connection.
        await self.dut.wlan_policy.start_client_connections()
        await self.wait_for_network(
            test_ssid, f_wlan_policy.ConnectionState.CONNECTING
        )
        await self.wait_for_network(
            test_ssid, f_wlan_policy.ConnectionState.CONNECTED
        )

        # Removing the network should initiate a auto-disconnection.
        await self.dut.wlan_policy.remove_all_networks()
        asserts.assert_equal(
            await self.dut.wlan_policy.get_saved_networks(), []
        )
        await self.wait_for_network(
            test_ssid,
            f_wlan_policy.ConnectionState.DISCONNECTED,
            f_wlan_policy.DisconnectStatus.CONNECTION_STOPPED,
        )

    async def test_save_network_with_client_connections_disabled(self) -> None:
        """Verify save_network() works without enabling client connections."""
        await self.dut.wlan_policy.stop_client_connections(
            wait_for_confirmation=True
        )

        test_ssid = AccessPointConfig.random_string()
        await self.dut.wlan_policy.save_network(
            test_ssid, f_wlan_policy.SecurityType.NONE
        )
        asserts.assert_equal(
            await self.dut.wlan_policy.get_saved_networks(),
            [
                NetworkConfig(
                    test_ssid, f_wlan_policy.SecurityType.NONE, "None", ""
                )
            ],
        )

        # Verify saving a network does not initiate an auto-connect.
        async for update in self.get_updates_until(timeout_sec=3):
            asserts.fail(f"Expected no updates, got {update}")

    async def test_connect_with_client_connections_disabled(self) -> None:
        """Verify connect() rejects without enabling client connections."""
        await self.dut.wlan_policy.stop_client_connections(
            wait_for_confirmation=True
        )

        test_ssid = AccessPointConfig.random_string()
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                test_ssid, f_wlan_policy.SecurityType.NONE
            )

        # Verify connect doesn't change client state.
        async for update in self.get_updates_until(timeout_sec=3):
            asserts.fail(f"Expected no updates, got {update}")

    async def test_remove_all_networks_with_client_connections_disabled(
        self,
    ) -> None:
        """Verify remove_all_networks() works without enabling client
        connections."""
        await self.dut.wlan_policy.stop_client_connections(
            wait_for_confirmation=True
        )

        await self.dut.wlan_policy.remove_all_networks()
        asserts.assert_equal(
            await self.dut.wlan_policy.get_saved_networks(),
            [],
        )

        test_ssid = AccessPointConfig.random_string()
        await self.dut.wlan_policy.save_network(
            test_ssid, f_wlan_policy.SecurityType.NONE
        )
        asserts.assert_equal(
            await self.dut.wlan_policy.get_saved_networks(),
            [
                NetworkConfig(
                    test_ssid, f_wlan_policy.SecurityType.NONE, "None", ""
                )
            ],
        )

        await self.dut.wlan_policy.remove_all_networks()
        asserts.assert_equal(
            await self.dut.wlan_policy.get_saved_networks(),
            [],
        )

    async def test_remove_network_with_client_connections_disabled(
        self,
    ) -> None:
        """Verify remove() works without enabling client connections."""
        test_ssid = AccessPointConfig.random_string()

        # Removing a network that doesn't exist shouldn't error.
        await self.dut.wlan_policy.forget_network(
            test_ssid, f_wlan_policy.SecurityType.NONE
        )
        asserts.assert_equal(
            await self.dut.wlan_policy.get_saved_networks(),
            [],
        )

        await self.dut.wlan_policy.save_network(
            test_ssid, f_wlan_policy.SecurityType.NONE
        )
        asserts.assert_equal(
            await self.dut.wlan_policy.get_saved_networks(),
            [
                NetworkConfig(
                    test_ssid, f_wlan_policy.SecurityType.NONE, "None", ""
                )
            ],
        )

        await self.dut.wlan_policy.forget_network(
            test_ssid, f_wlan_policy.SecurityType.NONE
        )
        asserts.assert_equal(
            await self.dut.wlan_policy.get_saved_networks(),
            [],
        )

    # TODO(http://b/339069764): Split WLAN utility functions out into a separate file
    async def get_updates_until(
        self, timeout_sec: float = 5
    ) -> AsyncIterator[ClientStateSummary]:
        """Iterate client state updates for a set duration."""
        end_time = time.time() + timeout_sec
        while time.time() < end_time:
            time_left = end_time - time.time()
            try:
                yield await self.dut.wlan_policy.get_update(timeout=time_left)
            except TimeoutError:
                return

    async def wait_for_update(
        self, expected_update: ClientStateSummary
    ) -> None:
        """Assert an update eventually matches the specified state."""
        last_updates: list[ClientStateSummary] = []

        async for update in self.get_updates_until(DEFAULT_GET_UPDATE_TIMEOUT):
            if update == expected_update:
                return
            last_updates.append(update)

        asserts.fail(
            f"Timed out waiting {DEFAULT_GET_UPDATE_TIMEOUT}s for client "
            f"state: {expected_update}\n"
            f"Last updates: {last_updates}"
        )

    async def wait_for_network(
        self,
        ssid: str,
        expected_state: f_wlan_policy.ConnectionState,
        expected_status: f_wlan_policy.DisconnectStatus | None = None,
        expected_client_state: f_wlan_policy.WlanClientState = f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
    ) -> None:
        """Assert the next update matches the specified network state."""
        await self.wait_for_update(
            ClientStateSummary(
                state=expected_client_state,
                networks=[
                    NetworkState(
                        NetworkIdentifier(
                            ssid, f_wlan_policy.SecurityType.NONE
                        ),
                        expected_state,
                        expected_status,
                    )
                ],
            )
        )


if __name__ == "__main__":
    test_runner.main()
