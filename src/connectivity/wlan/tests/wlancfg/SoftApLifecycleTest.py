# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for WLAN policy SoftAP lifecycle and state transitions."""

import logging

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from honeydew.affordances.connectivity.wlan.utils.types import (
    AccessPointState,
    NetworkIdentifier,
)
from mobly import asserts, test_runner
from openwrt_access_point.lib.access_point_config import AccessPointConfig

logger = logging.getLogger(__name__)


class SoftApLifecycleTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """Tests WLAN policy access point (SoftAP) lifecycle and state transitions."""

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy_ap.stop_all()
        await super().teardown_test()

    async def test_soft_ap_lifecycle_and_state_transitions(self) -> None:
        """Verify WLAN policy SoftAP transitions: STARTING -> ACTIVE -> frequency check -> snapshot -> stop."""
        await self.dut.wlan_policy_ap.stop_all()
        await self.dut.wlan_policy_ap.set_new_update_listener()
        asserts.assert_equal(
            await self.dut.wlan_policy_ap.get_update(),
            [],
        )

        test_ssid = AccessPointConfig.random_string(10)
        await self.dut.wlan_policy_ap.start(
            test_ssid,
            f_wlan_policy.SecurityType.NONE,
            None,
            f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
            f_wlan_policy.OperatingBand.ONLY_2_4_GHZ,
        )
        asserts.assert_equal(
            await self.dut.wlan_policy_ap.get_update(),
            [
                AccessPointState(
                    state=f_wlan_policy.OperatingState.STARTING,
                    mode=f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
                    band=f_wlan_policy.OperatingBand.ONLY_2_4_GHZ,
                    frequency=None,
                    clients=None,
                    id_=NetworkIdentifier(
                        ssid=test_ssid,
                        security_type=f_wlan_policy.SecurityType.NONE,
                    ),
                )
            ],
        )
        asserts.assert_equal(
            await self.dut.wlan_policy_ap.get_update(),
            [
                AccessPointState(
                    state=f_wlan_policy.OperatingState.ACTIVE,
                    mode=f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
                    band=f_wlan_policy.OperatingBand.ONLY_2_4_GHZ,
                    frequency=None,
                    clients=None,
                    id_=NetworkIdentifier(
                        ssid=test_ssid,
                        security_type=f_wlan_policy.SecurityType.NONE,
                    ),
                )
            ],
        )
        got_states = await self.dut.wlan_policy_ap.get_update()
        asserts.assert_greater_equal(got_states[0].frequency, 2412)  # channel 1
        asserts.assert_less_equal(got_states[0].frequency, 2484)  # channel 14
        asserts.assert_equal(
            got_states,
            [
                AccessPointState(
                    state=f_wlan_policy.OperatingState.ACTIVE,
                    mode=f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
                    band=f_wlan_policy.OperatingBand.ONLY_2_4_GHZ,
                    frequency=got_states[0].frequency,
                    clients=f_wlan_policy.ConnectedClientInformation(count=0),
                    id_=NetworkIdentifier(
                        ssid=test_ssid,
                        security_type=f_wlan_policy.SecurityType.NONE,
                    ),
                )
            ],
        )

        # Verify that a new update listener receives an accurate state snapshot
        await self.dut.wlan_policy_ap.set_new_update_listener()
        got_states = await self.dut.wlan_policy_ap.get_update()
        asserts.assert_is_not_none(got_states[0].frequency)
        asserts.assert_equal(
            got_states,
            [
                AccessPointState(
                    state=f_wlan_policy.OperatingState.ACTIVE,
                    mode=f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
                    band=f_wlan_policy.OperatingBand.ONLY_2_4_GHZ,
                    frequency=got_states[0].frequency,
                    clients=f_wlan_policy.ConnectedClientInformation(count=0),
                    id_=NetworkIdentifier(
                        ssid=test_ssid,
                        security_type=f_wlan_policy.SecurityType.NONE,
                    ),
                )
            ],
        )

        await self.dut.wlan_policy_ap.stop(
            test_ssid, f_wlan_policy.SecurityType.NONE, None
        )
        asserts.assert_equal(
            await self.dut.wlan_policy_ap.get_update(),
            [],
        )


if __name__ == "__main__":
    test_runner.main()
