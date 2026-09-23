#!/usr/bin/env python3
#
# Copyright 2026 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion.controllers.access_point import setup_ap
from antlion.controllers.ap_lib.hostapd_constants import (
    AP_DEFAULT_CHANNEL_5G,
    AP_SSID_LENGTH_5G,
)
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_5G_CHANNEL,
    AccessPointConfig,
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


# TODO(https://fxbug.dev/68956): Add security protocol check to mixed mode tests
# when info is available.
class WlanTargetSecurityTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """Tests Fuchsia's target security concept and security upgrading

    Testbed Requirements:
    * One Fuchsia device
    * One Whirlwind Access Point
    """

    async def setup_class(self) -> None:
        await super().setup_class()
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )

        if self.openwrt_aps:
            self.openwrt_ap = self.openwrt_aps[0]
        elif self.access_points:
            self.access_point = self.access_points[0]
            self.access_point.stop_all_aps()
        else:
            raise signals.TestAbortClass("Requires at least one access point")

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    def setup_ap(self, security: Security = SecurityOpen()) -> tuple[str, str]:
        """Sets up an AP using the provided security mode.

        Args:
            security: Security, security mode for AP
        Returns:
            Tuple, (ssid, password). Returns a password even if for open
                security, since non-open target securities require a credential
                to attempt a connection.
        """
        ssid = AccessPointConfig.random_string(AP_SSID_LENGTH_5G)
        # Length 13, so it can be used for WEP or WPA
        password = AccessPointConfig.random_string(13)

        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_5G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=security,
                                password=(
                                    password
                                    if security != SecurityOpen()
                                    else None
                                ),
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
        else:
            assert self.access_point is not None
            hostapd_security_mode = ConfigMapper.to_hostapd_security(security)

            security_profile = DeprecatedSecurity(
                security_mode=hostapd_security_mode, password=password
            )
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=AP_DEFAULT_CHANNEL_5G,
                ssid=ssid,
                security=security_profile,
            )

        return (ssid, password)

    # Open Security on AP
    async def test_associate_open_ap_with_open_target_security(self) -> None:
        ssid, _ = self.setup_ap()
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.NONE
        )
        await self.dut.wlan_policy.connect(
            ssid, f_wlan_policy.SecurityType.NONE
        )

    async def test_reject_open_ap_with_wep_target_security(self) -> None:
        ssid, password = self.setup_ap()
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WEP, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WEP
            )

    async def test_reject_open_ap_with_wpa_target_security(self) -> None:
        ssid, password = self.setup_ap()
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA
            )

    async def test_reject_open_ap_with_wpa2_target_security(self) -> None:
        ssid, password = self.setup_ap()
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA2, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA2
            )

    async def test_reject_open_ap_with_wpa3_target_security(self) -> None:
        ssid, password = self.setup_ap()
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA3, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA3
            )

    # WEP Security on AP
    async def test_reject_wep_ap_with_open_target_security(self) -> None:
        ssid, _ = self.setup_ap(SecurityWep())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.NONE
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.NONE
            )

    async def test_associate_wep_ap_with_wep_target_security(self) -> None:
        # TODO(b/490162087): Remove this skip once OpenWrt supports WEP security
        self.skip_if_wep_not_supported()
        ssid, password = self.setup_ap(SecurityWep())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WEP, password
        )
        await self.dut.wlan_policy.connect(ssid, f_wlan_policy.SecurityType.WEP)

    def skip_if_wep_not_supported(self) -> None:
        if self.openwrt_ap:
            raise signals.TestSkip("OpenWrt does not support WEP security")

    async def test_reject_wep_ap_with_wpa_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWep())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA
            )

    async def test_reject_wep_ap_with_wpa2_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWep())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA2, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA2
            )

    async def test_reject_wep_ap_with_wpa3_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWep())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA3, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA3
            )

    # WPA Security on AP
    async def test_reject_wpa_ap_with_open_target_security(self) -> None:
        ssid, _ = self.setup_ap(SecurityWpa())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.NONE
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.NONE
            )

    async def test_reject_wpa_ap_with_wep_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WEP, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WEP
            )

    async def test_associate_wpa_ap_with_wpa_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA, password
        )
        await self.dut.wlan_policy.connect(ssid, f_wlan_policy.SecurityType.WPA)

    async def test_reject_wpa_ap_with_wpa2_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA2, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA2
            )

    async def test_reject_wpa_ap_with_wpa3_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA3, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA3
            )

    # WPA2 Security on AP
    async def test_reject_wpa2_ap_with_open_target_security(self) -> None:
        ssid, _ = self.setup_ap(SecurityWpa2())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.NONE
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.NONE
            )

    async def test_reject_wpa2_ap_with_wep_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa2())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WEP, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WEP
            )

    async def test_associate_wpa2_ap_with_wpa_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa2())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA, password
        )
        await self.dut.wlan_policy.connect(ssid, f_wlan_policy.SecurityType.WPA)

    async def test_associate_wpa2_ap_with_wpa2_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa2())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA2, password
        )
        await self.dut.wlan_policy.connect(
            ssid, f_wlan_policy.SecurityType.WPA2
        )

    async def test_reject_wpa2_ap_with_wpa3_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa2())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA3, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA3
            )

    # WPA/WPA2 Security on AP
    async def test_reject_wpa_wpa2_ap_with_open_target_security(self) -> None:
        ssid, _ = self.setup_ap(SecurityWpaWpa2Mixed())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.NONE
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.NONE
            )

    async def test_reject_wpa_wpa2_ap_with_wep_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpaWpa2Mixed())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WEP, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WEP
            )

    async def test_associate_wpa_wpa2_ap_with_wpa_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpaWpa2Mixed())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA, password
        )
        await self.dut.wlan_policy.connect(ssid, f_wlan_policy.SecurityType.WPA)

    async def test_associate_wpa_wpa2_ap_with_wpa2_target_security(
        self,
    ) -> None:
        ssid, password = self.setup_ap(SecurityWpaWpa2Mixed())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA2, password
        )
        await self.dut.wlan_policy.connect(
            ssid, f_wlan_policy.SecurityType.WPA2
        )

    async def test_reject_wpa_wpa2_ap_with_wpa3_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpaWpa2Mixed())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA3, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA3
            )

    # WPA3 Security on AP
    async def test_reject_wpa3_ap_with_open_target_security(self) -> None:
        ssid, _ = self.setup_ap(SecurityWpa3())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.NONE
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.NONE
            )

    async def test_reject_wpa3_ap_with_wep_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa3())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WEP, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WEP
            )

    async def test_associate_wpa3_ap_with_wpa_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa3())
        # Expected failure to associate. WPA credentials for WPA3 was
        # temporarily disabled, see https://fxbug.dev/42166758 for context.
        # If this feature was re-enabled, please update this test's
        # expectation.
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA
            )

    async def test_associate_wpa3_ap_with_wpa2_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa3())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA2, password
        )
        await self.dut.wlan_policy.connect(
            ssid, f_wlan_policy.SecurityType.WPA2
        )

    async def test_associate_wpa3_ap_with_wpa3_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa3())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA3, password
        )
        await self.dut.wlan_policy.connect(
            ssid, f_wlan_policy.SecurityType.WPA3
        )

    # WPA2/WPA3 Security on AP
    async def test_reject_wpa2_wpa3_ap_with_open_target_security(self) -> None:
        ssid, _ = self.setup_ap(SecurityWpa2Wpa3Mixed())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.NONE
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.NONE
            )

    async def test_reject_wpa2_wpa3_ap_with_wep_target_security(self) -> None:
        ssid, password = self.setup_ap(SecurityWpa2Wpa3Mixed())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WEP, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WEP
            )

    async def test_associate_wpa2_wpa3_ap_with_wpa_target_security(
        self,
    ) -> None:
        ssid, password = self.setup_ap(SecurityWpa2Wpa3Mixed())
        # Expected failure to associate. WPA credentials for WPA3 was
        # temporarily disabled, see https://fxbug.dev/42166758 for context.
        # If this feature was re-enabled, please update this test's
        # expectation.
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA, password
        )
        with asserts.assert_raises(HoneydewWlanError):
            await self.dut.wlan_policy.connect(
                ssid, f_wlan_policy.SecurityType.WPA
            )

    async def test_associate_wpa2_wpa3_ap_with_wpa2_target_security(
        self,
    ) -> None:
        ssid, password = self.setup_ap(SecurityWpa2Wpa3Mixed())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA2, password
        )
        await self.dut.wlan_policy.connect(
            ssid, f_wlan_policy.SecurityType.WPA2
        )

    async def test_associate_wpa2_wpa3_ap_with_wpa3_target_security(
        self,
    ) -> None:
        ssid, password = self.setup_ap(SecurityWpa2Wpa3Mixed())
        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.WPA3, password
        )
        await self.dut.wlan_policy.connect(
            ssid, f_wlan_policy.SecurityType.WPA3
        )


if __name__ == "__main__":
    test_runner.main()
