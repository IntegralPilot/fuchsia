#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import logging
from dataclasses import dataclass
from datetime import datetime, timedelta

import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from antlion.controllers.access_point import setup_ap
from antlion.controllers.ap_lib import hostapd_constants
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point import Radio
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    DEFAULT_5G_CHANNEL,
    AccessPointConfig,
    Band,
    BssChannel,
    BssSettings,
    RadioConfig,
    Security,
    SecurityOpen,
    SecurityWep,
    SecurityWpa,
    SecurityWpa2,
    SecurityWpa2Wpa3Mixed,
    SecurityWpa3,
    SecurityWpaWpa2Mixed,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)

logger = logging.getLogger(__name__)


@dataclass
class TestParams:
    dut_security: Security
    original_security: Security
    original_channel: BssChannel
    target_security: Security
    target_channel: BssChannel
    expect_roam: bool


_DUT_SECURITIES: frozenset[Security] = frozenset(
    [
        SecurityOpen(),
        SecurityWep(),
        SecurityWpa(),
        SecurityWpa2(),
        SecurityWpa3(),
    ]
)

_AP_SECURITIES: frozenset[Security] = _DUT_SECURITIES | frozenset(
    [
        SecurityWpaWpa2Mixed(),
        SecurityWpa2Wpa3Mixed(),
    ]
)

_DUT_SECURITY_TO_COMPATIBLE_AP_SECURITIES: dict[
    Security, frozenset[Security]
] = {
    SecurityOpen(): frozenset([SecurityOpen()]),
    SecurityWep(): frozenset([SecurityWep()]),
    SecurityWpa(): frozenset([SecurityWpa(), SecurityWpaWpa2Mixed()]),
    SecurityWpa2(): frozenset(
        [
            SecurityWpa2(),
            SecurityWpaWpa2Mixed(),
            SecurityWpa2Wpa3Mixed(),
        ]
    ),
    SecurityWpa3(): frozenset([SecurityWpa3(), SecurityWpa2Wpa3Mixed()]),
}


class WlanPolicyInitiatedRoamTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """Tests Fuchsia's WLAN Policy-initiated roam support.

    Testbed Requirements:
    * One Fuchsia device
    * One Whirlwind or OpenWrt access point
    """

    phy: wlan_core.Phy
    client_iface: wlan_core.ClientIface

    async def pre_run(self) -> None:
        test_args: list[tuple[TestParams]] = []

        for (
            dut_security,
            compatible_ap_securities,
        ) in _DUT_SECURITY_TO_COMPATIBLE_AP_SECURITIES.items():
            for ap_security in compatible_ap_securities:
                # Same compatible security mode on both APs, 2.4 GHz to 5 GHz.
                test_args.append(
                    (
                        TestParams(
                            dut_security=dut_security,
                            original_security=ap_security,
                            original_channel=DEFAULT_2G_CHANNEL,
                            target_security=ap_security,
                            target_channel=DEFAULT_5G_CHANNEL,
                            expect_roam=True,
                        ),
                    )
                )

                # Same compatible security mode on both APs, 5 GHz to 2.4 GHz.
                test_args.append(
                    (
                        TestParams(
                            dut_security=dut_security,
                            original_security=ap_security,
                            original_channel=DEFAULT_5G_CHANNEL,
                            target_security=ap_security,
                            target_channel=DEFAULT_2G_CHANNEL,
                            expect_roam=True,
                        ),
                    )
                )

                # Test incompatible roams, which should all fail.
                incompatible_securities = (
                    _AP_SECURITIES - compatible_ap_securities
                )
                for incompatible_security in incompatible_securities:
                    test_args.append(
                        (
                            TestParams(
                                dut_security=dut_security,
                                original_security=ap_security,
                                original_channel=DEFAULT_2G_CHANNEL,
                                target_security=incompatible_security,
                                target_channel=DEFAULT_5G_CHANNEL,
                                expect_roam=False,
                            ),
                        ),
                    )

        def generate_roam_test_name(test: TestParams) -> str:
            expected = "roams" if test.expect_roam else "does_not_roam"
            return f"test_{test.dut_security.uci_encryption}_dut_{expected}_from_{test.original_security.uci_encryption}_{test.original_channel.band}_to_{test.target_security.uci_encryption}_{test.target_channel.band}"

        self.generate_tests(
            test_logic=self._test_logic,
            name_func=generate_roam_test_name,
            arg_sets=test_args,
        )

    async def setup_class(self) -> None:
        await super().setup_class()

        self.phy = await self.dut.wlan_core.ensure_single_phy()

        if not self.openwrt_ap and not self.access_point:
            raise signals.TestAbortClass("Requires at least one access point")

    async def teardown_class(self) -> None:
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_class()

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()
        client_ifaces = await self.phy.get_client_ifaces()
        asserts.assert_equal(
            len(client_ifaces),
            1,
            f"Expected exactly 1 client interface on PHY, got {len(client_ifaces)}",
        )
        self.client_iface = client_ifaces[0]

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    async def _test_logic(self, test: TestParams) -> None:
        """Setup the APs, associate a DUT, and slowly reduce AP signal strength until roam.

        Args:
            test: Test parameters
        """
        ssid = AccessPointConfig.random_string(
            hostapd_constants.AP_SSID_LENGTH_2G
        )
        # Length 13, so it can be used for WEP or WPA
        password = AccessPointConfig.random_string(13)
        original_password = (
            password
            if not isinstance(test.original_security, SecurityOpen)
            else None
        )
        target_password = (
            password
            if not isinstance(test.target_security, SecurityOpen)
            else None
        )
        dut_password = (
            password
            if not isinstance(test.dut_security, SecurityOpen)
            else None
        )

        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=test.original_channel,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=test.original_security,
                                password=original_password,
                            )
                        ],
                    ),
                    RadioConfig.generate(
                        channel=test.target_channel,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=test.target_security,
                                password=target_password,
                            )
                        ],
                    ),
                ]
            )
            self.openwrt_ap.configure_wifi(config)

            target_radio = (
                Radio.RADIO_5G
                if test.target_channel.band == Band.BAND_5G
                else Radio.RADIO_2G
            )
            # Disable target radio immediately so client connects to original band
            self.openwrt_ap.disable_radio(target_radio)
        elif self.access_point:
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=test.original_channel.number,
                ssid=ssid,
                security=DeprecatedSecurity(
                    security_mode=ConfigMapper.to_hostapd_security(
                        test.original_security
                    ),
                    password=original_password,
                ),
            )
        await self.dut.wlan_policy.save_network(
            ssid,
            test.dut_security.to_fidl_wlan_policy(),
            target_pwd=dut_password,
        )
        await self.dut.wlan_policy.connect(
            ssid,
            test.dut_security.to_fidl_wlan_policy(),
        )

        # Verify that DUT is actually associated (as seen from AP).
        client_mac = await self.client_iface.get_mac_address()

        # Verify that DUT is actually associated (as seen from AP).
        original_identifier = ""
        target_identifier = ""

        if self.openwrt_ap:
            if test.original_channel.band == Band.BAND_2G:
                original_identifier = self.openwrt_ap.wlan_2g_interface
            else:
                original_identifier = self.openwrt_ap.wlan_5g_interface
        elif self.access_point:
            if test.original_channel.band == Band.BAND_2G:
                original_identifier = self.access_point.wlan_2g
            elif test.original_channel.band == Band.BAND_5G:
                original_identifier = self.access_point.wlan_5g

        if self.openwrt_ap:
            try:
                status = self.openwrt_ap.get_sta_status(
                    client_mac, test.original_channel.band
                )
                is_assoc = status.assoc
            except RuntimeError:
                is_assoc = False
        else:
            assert self.access_point is not None
            is_assoc = self.access_point.sta_associated(
                original_identifier, client_mac
            )

        asserts.assert_true(
            is_assoc,
            f"DUT is not associated on the {test.original_channel.band} band",
        )

        # Setup target AP.
        if self.openwrt_ap:
            target_radio = (
                Radio.RADIO_5G
                if test.target_channel.band == Band.BAND_5G
                else Radio.RADIO_2G
            )
            self.openwrt_ap.enable_radio(target_radio)
        elif self.access_point:
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=test.target_channel.number,
                ssid=ssid,
                security=DeprecatedSecurity(
                    security_mode=ConfigMapper.to_hostapd_security(
                        test.target_security
                    ),
                    password=target_password,
                ),
            )

        if self.openwrt_ap:
            if test.target_channel.band == Band.BAND_2G:
                target_identifier = self.openwrt_ap.wlan_2g_interface
            else:
                target_identifier = self.openwrt_ap.wlan_5g_interface
        elif self.access_point:
            if test.target_channel.band == Band.BAND_2G:
                target_identifier = self.access_point.wlan_2g
            elif test.target_channel.band == Band.BAND_5G:
                target_identifier = self.access_point.wlan_5g

        FULL_POWER_DBM = 23
        current_dbm = FULL_POWER_DBM
        NUM_ITERATIONS = 10
        PERIOD_S = 10

        for id in (original_identifier, target_identifier):
            # Reset back to full power.
            if self.openwrt_ap:
                self.openwrt_ap.reset_txpower(id)
            elif self.access_point:
                self.access_point.iwconfig.ap_iwconfig(
                    id, f"txpower {FULL_POWER_DBM}"
                )
                self.access_point.iwconfig.ap_iwconfig(id, "txpower auto")
        for i in range(NUM_ITERATIONS):
            logger.info(
                f"Iteration {i + 1}/{NUM_ITERATIONS}: "
                f"Reducing power from {current_dbm} dBm."
            )
            # Reduce power, but with a floor of 1 dBm.
            current_dbm = max(current_dbm // 2, 1)
            if self.openwrt_ap:
                self.openwrt_ap.set_txpower(original_identifier, current_dbm)
            elif self.access_point:
                self.access_point.iwconfig.ap_iwconfig(
                    original_identifier, f"txpower {current_dbm}"
                )

            period_deadline = datetime.now() + timedelta(seconds=PERIOD_S)
            while datetime.now() < period_deadline:
                # Check for STA on destination, and if it has roamed, end the test.
                if test.expect_roam:
                    if self.openwrt_ap:
                        try:
                            status = self.openwrt_ap.get_sta_status(
                                client_mac, test.target_channel.band
                            )
                            is_authorized = status.authorized
                        except RuntimeError:
                            is_authorized = False
                    else:
                        assert self.access_point is not None
                        is_authorized = self.access_point.sta_authorized(
                            target_identifier, client_mac
                        )

                    if is_authorized:
                        logger.info(
                            "DUT successfully roamed and is authorized on target BSS!"
                        )
                        break
                    # We want to detect if DUT disconnected from the original BSS without roaming to the
                    # target BSS. Specifically, we want to avoid a false positive if DUT does a full
                    # disconnect from the original BSS followed by a regular connect to the target BSS,
                    # rather than roaming between them. This is not a perfect mechanism to detect this
                    # case, but it suffices for manually run tests. Automated tests will need a better
                    # way to detect this scenario.
                    # TODO(https://fxbug.dev/359966771): Surface intermediate states to Antlion.
                    if self.openwrt_ap:
                        try:
                            status = self.openwrt_ap.get_sta_status(
                                client_mac, test.original_channel.band
                            )
                            is_assoc = status.assoc
                        except RuntimeError:
                            is_assoc = False
                    else:
                        assert self.access_point is not None
                        is_assoc = self.access_point.sta_associated(
                            original_identifier, client_mac
                        )

                    if not is_assoc:
                        raise signals.TestFailure(
                            "DUT left original BSS without roaming to target BSS"
                        )
                await asyncio.sleep(0.25)

        if test.expect_roam:
            # Verify that DUT roamed (as seen from AP).
            if self.openwrt_ap:
                try:
                    status = self.openwrt_ap.get_sta_status(
                        client_mac, test.target_channel.band
                    )
                    is_auth = status.auth
                    is_assoc = status.assoc
                    is_authorized = status.authorized
                except RuntimeError:
                    is_auth = False
                    is_assoc = False
                    is_authorized = False
            else:
                assert self.access_point is not None
                is_auth = self.access_point.sta_authenticated(
                    target_identifier, client_mac
                )
                is_assoc = self.access_point.sta_associated(
                    target_identifier, client_mac
                )
                is_authorized = self.access_point.sta_authorized(
                    target_identifier, client_mac
                )

            asserts.assert_true(
                is_auth,
                f"DUT is not authenticated on the {test.target_channel.band} band",
            )
            asserts.assert_true(
                is_assoc,
                f"DUT is not associated on the {test.target_channel.band} band",
            )
            asserts.assert_true(
                is_authorized, "DUT is not 802.1X authorized on the 5GHz band"
            )
        else:
            # DUT should have stayed on the original BSS.
            if self.openwrt_ap:
                try:
                    status = self.openwrt_ap.get_sta_status(
                        client_mac, test.original_channel.band
                    )
                    is_auth = status.auth
                except RuntimeError:
                    is_auth = False
            else:
                assert self.access_point is not None
                is_auth = self.access_point.sta_authenticated(
                    original_identifier, client_mac
                )

            asserts.assert_true(
                is_auth,
                f"DUT is not authenticated on the {test.original_channel.band.name} band",
            )


if __name__ == "__main__":
    test_runner.main()
