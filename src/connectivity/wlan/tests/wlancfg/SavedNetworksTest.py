#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion.controllers.access_point import setup_ap
from antlion.controllers.ap_lib import hostapd_constants
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.utils import rand_ascii_str, rand_hex_str
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from honeydew.affordances.connectivity.wlan.utils.types import NetworkConfig
from mobly import asserts, signals, test_runner
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    Security,
    SecurityOpen,
    SecurityWpa2,
    SecurityWpa3,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)

PSK_LEN = 64
CREDENTIAL_TYPE_PSK = "Psk"
CREDENTIAL_TYPE_NONE = "None"
CREDENTIAL_TYPE_PASSWORD = "Password"
CREDENTIAL_VALUE_NONE = ""


class SavedNetworksTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """WLAN policy commands test class.

    A test that saves various networks and verifies the behavior of save, get, and
    remove through the ClientController API of WLAN policy.

    Test Bed Requirement:
    * One Fuchsia device
    * One Access Point
    """

    async def setup_class(self) -> None:
        await super().setup_class()
        self.log = logging.getLogger()
        if self.access_point:
            self.access_point.stop_all_aps()

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()
        if self.access_point:
            self.access_point.stop_all_aps()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        await super().teardown_test()

    async def teardown_class(self) -> None:
        if hasattr(self, "access_point") and self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_class()

    async def _has_saved_network(self, network: NetworkConfig) -> bool:
        """Verify that the network is present in saved networks.

        Args:
            network: Network to check for.

        Returns:
            True if network is found in saved networks, otherwise False.
        """
        networks = await self.dut.wlan_policy.get_saved_networks()
        return network in networks

    def _start_ap(
        self,
        ssid: str,
        security: Security,
        password: str | None = None,
    ) -> None:
        """Starts an access point.

        Args:
            ssid: The SSID of the network to broadcast
            security_type: The security type of the network to be broadcasted
            password: The password to connect to the broadcasted network. The password
                is ignored if security type is none.

        Raises:
            EnvironmentError if it fails to set up AP for test.
        """
        # Put together the security configuration of the network to be broadcasted.
        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_2G_CHANNEL,
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
            # Create an AP with default values other than the specified values.
            deprecated_security = ConfigMapper.to_hostapd_security(security)
            setup_ap(
                self.access_point,
                "whirlwind",
                hostapd_constants.AP_DEFAULT_CHANNEL_2G,
                ssid,
                security=DeprecatedSecurity(
                    deprecated_security,
                    password,
                ),
            )
        else:
            self.log.error(
                "No access point available for test, please check config"
            )
            raise EnvironmentError("Failed to set up AP for test")

    async def test_open_network_with_password(self) -> None:
        """Save an open network with a password and verify that it fails to save."""
        test_network = NetworkConfig(
            rand_ascii_str(10),
            f_wlan_policy.SecurityType.NONE,
            CREDENTIAL_TYPE_NONE,
            rand_ascii_str(8),
        )

        try:
            await self.dut.wlan_policy.save_network(
                test_network.ssid,
                test_network.security_type,
                test_network.credential_value,
            )
            asserts.fail("Unexpectedly succeeded to save network")
        except HoneydewWlanError:
            networks = await self.dut.wlan_policy.get_saved_networks()
            if test_network in networks:
                asserts.fail("Got an unexpected saved network")
            # Successfully failed to save network.
            return

        asserts.fail("Failed to get error saving bad network")

    async def test_open_network(self) -> None:
        """Save an open network and verify presence."""
        test_network = NetworkConfig(
            rand_ascii_str(10),
            f_wlan_policy.SecurityType.NONE,
            CREDENTIAL_TYPE_NONE,
            CREDENTIAL_VALUE_NONE,
        )

        await self.dut.wlan_policy.save_network(
            test_network.ssid,
            test_network.security_type,
            test_network.credential_value,
        )
        if not await self._has_saved_network(test_network):
            asserts.fail("Saved network not present")

    async def test_network_with_psk(self) -> None:
        """Save a network with a PSK and verify presence.

        PSK are translated from hex to bytes when saved, and when returned by
        get_saved_networks it will be lower case.
        """
        test_network = NetworkConfig(
            rand_ascii_str(11),
            f_wlan_policy.SecurityType.WPA2,
            CREDENTIAL_TYPE_PSK,
            rand_hex_str(PSK_LEN).lower(),
        )

        await self.dut.wlan_policy.save_network(
            test_network.ssid,
            test_network.security_type,
            test_network.credential_value,
        )
        if not await self._has_saved_network(test_network):
            asserts.fail("Saved network not present")

    async def test_wep_network(self) -> None:
        """Save a wep network and verify presence."""
        test_network = NetworkConfig(
            rand_ascii_str(12),
            f_wlan_policy.SecurityType.WEP,
            CREDENTIAL_TYPE_PASSWORD,
            rand_ascii_str(13),
        )

        await self.dut.wlan_policy.save_network(
            test_network.ssid,
            test_network.security_type,
            test_network.credential_value,
        )
        if not await self._has_saved_network(test_network):
            asserts.fail("Saved network not present")

    async def test_wpa2_network(self) -> None:
        """Save a wpa2 network and verify presence."""
        test_network = NetworkConfig(
            rand_ascii_str(9),
            f_wlan_policy.SecurityType.WPA2,
            CREDENTIAL_TYPE_PASSWORD,
            rand_ascii_str(15),
        )

        await self.dut.wlan_policy.save_network(
            test_network.ssid,
            test_network.security_type,
            test_network.credential_value,
        )
        if not await self._has_saved_network(test_network):
            asserts.fail("Saved network not present")

    async def test_wpa_network(self) -> None:
        """Save a wpa network and verify presence."""
        test_network = NetworkConfig(
            rand_ascii_str(16),
            f_wlan_policy.SecurityType.WPA,
            CREDENTIAL_TYPE_PASSWORD,
            rand_ascii_str(9),
        )

        await self.dut.wlan_policy.save_network(
            test_network.ssid,
            test_network.security_type,
            test_network.credential_value,
        )
        if not await self._has_saved_network(test_network):
            asserts.fail("Saved network not present")

    async def test_wpa3_network(self) -> None:
        """Save a wpa3 network and verify presence."""
        test_network = NetworkConfig(
            rand_ascii_str(9),
            f_wlan_policy.SecurityType.WPA3,
            CREDENTIAL_TYPE_PASSWORD,
            rand_ascii_str(15),
        )

        await self.dut.wlan_policy.save_network(
            test_network.ssid,
            test_network.security_type,
            test_network.credential_value,
        )
        if not await self._has_saved_network(test_network):
            asserts.fail("Saved network not present")

    async def test_save_network_persists(self) -> None:
        """Save a network and verify after reboot network is present."""
        test_network = NetworkConfig(
            rand_ascii_str(10),
            f_wlan_policy.SecurityType.WPA2,
            CREDENTIAL_TYPE_PASSWORD,
            rand_ascii_str(10),
        )

        await self.dut.wlan_policy.save_network(
            test_network.ssid,
            test_network.security_type,
            test_network.credential_value,
        )

        if not await self._has_saved_network(test_network):
            asserts.fail("Saved network not present")

        await self.dut.reboot()

        if not await self._has_saved_network(test_network):
            asserts.fail("Saved network did not persist through reboot")

    async def test_same_ssid_diff_security(self) -> None:
        """Save two networks with the same ssids but different security types.

        Both networks should be saved and present in network state since they have
        different security types and therefore different network identifiers.
        """
        ssid = rand_ascii_str(19)
        test_network_wpa2 = NetworkConfig(
            ssid,
            f_wlan_policy.SecurityType.WPA2,
            CREDENTIAL_TYPE_PASSWORD,
            rand_ascii_str(12),
        )
        test_network_open = NetworkConfig(
            ssid,
            f_wlan_policy.SecurityType.NONE,
            CREDENTIAL_TYPE_NONE,
            CREDENTIAL_VALUE_NONE,
        )

        await self.dut.wlan_policy.save_network(
            test_network_wpa2.ssid,
            test_network_wpa2.security_type,
            test_network_wpa2.credential_value,
        )

        await self.dut.wlan_policy.save_network(
            test_network_open.ssid,
            test_network_open.security_type,
            test_network_open.credential_value,
        )

        if not (
            await self._has_saved_network(test_network_wpa2)
            and await self._has_saved_network(test_network_open)
        ):
            asserts.fail("Both saved networks not present")

    async def test_remove_disconnects(self) -> None:
        """Connect to network, remove it while still connected, and verify disconnect.

        This test requires a wpa2 network. Remove all other networks first so that we
        don't auto connect to them.
        """
        test_network = NetworkConfig(
            rand_ascii_str(10),
            f_wlan_policy.SecurityType.WPA2,
            CREDENTIAL_TYPE_PASSWORD,
            rand_ascii_str(10),
        )

        self._start_ap(
            test_network.ssid, SecurityWpa2(), test_network.credential_value
        )

        await self.dut.wlan_policy.wait_for_no_connections()
        # Make sure client connections are enabled
        await self.dut.wlan_policy.start_client_connections()
        await self.dut.wlan_policy.wait_for_client_state(
            f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED
        )
        # Save and verify we connect to network
        await self.dut.wlan_policy.save_network(
            test_network.ssid,
            test_network.security_type,
            test_network.credential_value,
        )

        await self.dut.wlan_policy.wait_for_network_state(
            test_network.ssid, f_wlan_policy.ConnectionState.CONNECTED
        )
        # Remove network and verify we disconnect
        await self.dut.wlan_policy.forget_network(
            test_network.ssid,
            test_network.security_type,
        )
        try:
            await self.dut.wlan_policy.wait_for_no_connections()
        except HoneydewWlanError as e:
            raise signals.TestFailure("Failed to remove network") from e

    async def test_auto_connect_open(self) -> None:
        """Save an open network and verify it auto connects.

        Start up AP with an open network and verify that the client auto connects to
        that network after we save it.
        """
        test_network = NetworkConfig(
            rand_ascii_str(10),
            f_wlan_policy.SecurityType.NONE,
            CREDENTIAL_TYPE_NONE,
            CREDENTIAL_VALUE_NONE,
        )

        self._start_ap(
            test_network.ssid, SecurityOpen(), test_network.credential_value
        )

        await self.dut.wlan_policy.wait_for_no_connections()
        # Make sure client connections are enabled
        await self.dut.wlan_policy.start_client_connections()

        await self.dut.wlan_policy.wait_for_client_state(
            f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED
        )
        # Save the network and make sure that we see the device auto connect to it.
        await self.dut.wlan_policy.save_network(
            test_network.ssid, test_network.security_type
        )
        try:
            await self.dut.wlan_policy.wait_for_network_state(
                test_network.ssid, f_wlan_policy.ConnectionState.CONNECTED
            )
        except HoneydewWlanError as e:
            raise signals.TestFailure(
                "network is not in connected state"
            ) from e

    async def test_auto_connect_wpa3(self) -> None:
        """Save an wpa3 network and verify it auto connects.

        Start up AP with a wpa3 network and verify that the client auto connects to
        that network after we save it.
        """
        test_network = NetworkConfig(
            rand_ascii_str(10),
            f_wlan_policy.SecurityType.WPA3,
            CREDENTIAL_TYPE_PASSWORD,
            rand_ascii_str(10),
        )

        self._start_ap(
            test_network.ssid, SecurityWpa3(), test_network.credential_value
        )

        await self.dut.wlan_policy.wait_for_no_connections()
        # Make sure client connections are enabled
        await self.dut.wlan_policy.start_client_connections()
        await self.dut.wlan_policy.wait_for_client_state(
            f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED
        )
        # Save the network and make sure that we see the device auto connect to it.
        await self.dut.wlan_policy.save_network(
            test_network.ssid,
            f_wlan_policy.SecurityType.WPA3,
            test_network.credential_value,
        )
        try:
            await self.dut.wlan_policy.wait_for_network_state(
                test_network.ssid, f_wlan_policy.ConnectionState.CONNECTED
            )
        except HoneydewWlanError as e:
            raise signals.TestFailure(
                "network is not in connected state"
            ) from e


if __name__ == "__main__":
    test_runner.main()
