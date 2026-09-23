#!/usr/bin/env python3
#
# Copyright 2026 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
from datetime import datetime, timedelta, timezone
from typing import FrozenSet

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from antlion.controllers.access_point import setup_ap
from antlion.controllers.ap_lib import hostapd_constants
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.controllers.ap_lib.radio_measurement import (
    BssidInformation,
    BssidInformationCapabilities,
    NeighborReportElement,
    PhyType,
)
from antlion.controllers.ap_lib.wireless_network_management import (
    BssTransitionCandidateList,
    BssTransitionManagementRequest,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
    MacAddress,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point import Radio, StationStatus
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    DEFAULT_5G_CHANNEL,
    AccessPointConfig,
    Band,
    BssSettings,
    RadioConfig,
    SecurityOpen,
)
from openwrt_access_point.lib.extended_capabilities import ExtendedCapabilities
from openwrt_access_point.lib.uci_bss_options import UciBssOptions


# Antlion can see (via the wlan_features config directive) whether WNM features
# are enabled, and runs or skips tests depending on presence of WNM features.
class WlanWirelessNetworkManagementTest(
    fuchsia_wlan_base_test.FuchsiaWlanBaseTest
):
    """Tests Fuchsia's Wireless Network Management (AKA 802.11v) support.

    Testbed Requirements:
    * One Fuchsia device
    * One Whirlwind access point
    """

    phy: wlan_core.Phy
    client_iface: wlan_core.ClientIface

    async def setup_class(self) -> None:
        await super().setup_class()

        # Set country code US to support 5G bands for roaming test.
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )

        self.phy = await self.dut.wlan_core.ensure_single_phy()

        if self.openwrt_aps:
            self.openwrt_ap = self.openwrt_aps[0]
        elif self.access_points:
            self.access_point = self.access_points[0]
            self.access_point.stop_all_aps()
        else:
            raise signals.TestAbortClass("Requires at least one access point")

    def get_single_sta_ext_capabilities(
        self, mac: MacAddress, band: Band
    ) -> ExtendedCapabilities:
        """Gets extended capabilities and asserts there is only one interface."""
        assert self.openwrt_ap is not None, "openwrt_ap is not initialized"
        ext_caps_dict = self.openwrt_ap.get_sta_extended_capabilities(mac, band)
        assert (
            len(ext_caps_dict) == 1
        ), f"Expected station on exactly one interface, but found: {list(ext_caps_dict.keys())}"
        return list(ext_caps_dict.values())[0]

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

    def setup_ap(
        self,
        ssid: str,
        security: DeprecatedSecurity | None = None,
        additional_ap_parameters: dict[str, int] | None = None,
        channel: int = hostapd_constants.AP_DEFAULT_CHANNEL_2G,
        wnm_features: FrozenSet[hostapd_constants.WnmFeature] = frozenset(),
    ) -> None:
        """Sets up an AP using the provided parameters.

        Args:
            ssid: SSID for the AP.
            security: security config for AP, defaults to None (open network
                with no password).
            additional_ap_parameters: A dictionary of parameters that can be set
                directly in the hostapd config file.
            channel: which channel number to set the AP to (default is
                AP_DEFAULT_CHANNEL_2G).
            wnm_features: Wireless Network Management features to enable
                (default is no WNM features).
        """
        assert self.access_point is not None
        setup_ap(
            access_point=self.access_point,
            profile_name="whirlwind",
            channel=channel,
            ssid=ssid,
            security=security,
            additional_ap_parameters=additional_ap_parameters,
            wnm_features=wnm_features,
        )

    async def test_bss_transition_is_not_advertised_when_ap_supported_dut_unsupported(
        self,
    ) -> None:
        ssid = AccessPointConfig.random_string(
            hostapd_constants.AP_SSID_LENGTH_2G
        )
        wnm_features = frozenset(
            [hostapd_constants.WnmFeature.BSS_TRANSITION_MANAGEMENT]
        )
        if self.openwrt_ap:
            channel = DEFAULT_2G_CHANNEL
            config = AccessPointConfig(
                radios=[
                    RadioConfig(
                        channel=channel,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=SecurityOpen(),
                                custom_uci_options=UciBssOptions(
                                    bss_transition=True
                                ),
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
        elif self.access_point:
            self.setup_ap(ssid, wnm_features=wnm_features)

        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.NONE
        )
        await self.dut.wlan_policy.connect(
            ssid, f_wlan_policy.SecurityType.NONE
        )

        client_mac = await self.client_iface.get_mac_address()
        # Verify that DUT is actually associated (as seen from AP).

        if self.openwrt_ap:
            sta_status = self.openwrt_ap.get_sta_status(
                client_mac, band=Band.BAND_2G
            )
            asserts.assert_true(
                sta_status.assoc,
                "DUT is not associated on the 2.4GHz band",
            )
            ext_capabilities = self.get_single_sta_ext_capabilities(
                client_mac, band=Band.BAND_2G
            )
            asserts.assert_false(
                ext_capabilities.bss_transition,
                "DUT is incorrectly advertising BSS Transition Management support",
            )
        elif self.access_point:
            asserts.assert_true(
                self.access_point.sta_associated(
                    self.access_point.wlan_2g, client_mac
                ),
                "DUT is not associated on the 2.4GHz band",
            )
            legacy_ext_capabilities = (
                self.access_point.get_sta_extended_capabilities(
                    self.access_point.wlan_2g, client_mac
                )
            )
            asserts.assert_false(
                legacy_ext_capabilities.bss_transition,
                "DUT is incorrectly advertising BSS Transition Management support",
            )

    async def test_wnm_sleep_mode_is_not_advertised_when_ap_supported_dut_unsupported(
        self,
    ) -> None:
        ssid = AccessPointConfig.random_string(
            hostapd_constants.AP_SSID_LENGTH_2G
        )
        wnm_features = frozenset([hostapd_constants.WnmFeature.WNM_SLEEP_MODE])
        if self.openwrt_ap:
            channel = DEFAULT_2G_CHANNEL
            config = AccessPointConfig(
                radios=[
                    RadioConfig(
                        channel=channel,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=SecurityOpen(),
                                custom_uci_options=UciBssOptions(
                                    wnm_sleep_mode=True
                                ),
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
        elif self.access_point:
            self.setup_ap(ssid, wnm_features=wnm_features)

        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.NONE
        )
        await self.dut.wlan_policy.connect(
            ssid, f_wlan_policy.SecurityType.NONE
        )

        client_mac = await self.client_iface.get_mac_address()
        # Verify that DUT is actually associated (as seen from AP).
        if self.openwrt_ap:
            sta_status = self.openwrt_ap.get_sta_status(
                client_mac, band=Band.BAND_2G
            )
            asserts.assert_true(
                sta_status.assoc, "DUT is not associated on the 2.4GHz band"
            )
            ext_capabilities = self.get_single_sta_ext_capabilities(
                client_mac, band=Band.BAND_2G
            )
            asserts.assert_false(
                ext_capabilities.wnm_sleep_mode,
                "DUT is incorrectly advertising WNM Sleep Mode support",
            )
        elif self.access_point:
            asserts.assert_true(
                self.access_point.sta_associated(
                    self.access_point.wlan_2g, client_mac
                ),
                "DUT is not associated on the 2.4GHz band",
            )
            legacy_ext_capabilities = (
                self.access_point.get_sta_extended_capabilities(
                    self.access_point.wlan_2g, client_mac
                )
            )
            asserts.assert_false(
                legacy_ext_capabilities.wnm_sleep_mode,
                "DUT is incorrectly advertising WNM Sleep Mode support",
            )

    async def test_btm_req_ignored_dut_unsupported(self) -> None:
        ssid = AccessPointConfig.random_string(
            hostapd_constants.AP_SSID_LENGTH_2G
        )
        wnm_features = frozenset(
            [hostapd_constants.WnmFeature.BSS_TRANSITION_MANAGEMENT]
        )
        # Setup 2.4 GHz AP.
        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig(
                        channel=DEFAULT_2G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=SecurityOpen(),
                                custom_uci_options=UciBssOptions(
                                    bss_transition=True
                                ),
                            )
                        ],
                    ),
                    RadioConfig(
                        channel=DEFAULT_5G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=SecurityOpen(),
                                custom_uci_options=UciBssOptions(
                                    bss_transition=True
                                ),
                            )
                        ],
                    ),
                ]
            )
            self.openwrt_ap.configure_wifi(config)
            # Disable 5 GHz radio immediately so client connects to 2G
            self.openwrt_ap.disable_radio(Radio.RADIO_5G)
        elif self.access_point:
            self.setup_ap(
                ssid,
                channel=hostapd_constants.AP_DEFAULT_CHANNEL_2G,
                wnm_features=wnm_features,
            )

        await self.dut.wlan_policy.save_network(
            ssid, f_wlan_policy.SecurityType.NONE
        )
        await self.dut.wlan_policy.connect(
            ssid, f_wlan_policy.SecurityType.NONE
        )

        # Verify that DUT is actually associated (as seen from AP).
        client_mac = await self.client_iface.get_mac_address()
        if self.openwrt_ap:
            sta_status = self.openwrt_ap.get_sta_status(
                client_mac, band=Band.BAND_2G
            )
            asserts.assert_true(
                sta_status.assoc, "DUT is not associated on the 2.4GHz band"
            )
        elif self.access_point:
            asserts.assert_true(
                self.access_point.sta_associated(
                    self.access_point.wlan_2g, client_mac
                ),
                "DUT is not associated on the 2.4GHz band",
            )

        # Setup 5 GHz AP with same SSID.
        if self.openwrt_ap:
            self.openwrt_ap.enable_radio(Radio.RADIO_5G)
        elif self.access_point:
            self.setup_ap(
                ssid,
                channel=hostapd_constants.AP_DEFAULT_CHANNEL_5G,
                wnm_features=wnm_features,
            )

        # Construct a BTM request.
        if self.openwrt_ap:
            dest_bssid = self.openwrt_ap.get_bssid_from_ssid(ssid, Band.BAND_5G)
        else:
            assert self.access_point is not None
            dest_bssid = self.access_point.get_bssid_from_ssid(
                ssid, hostapd_constants.BandType.BAND_5G
            )
        dest_bssid_info = BssidInformation(
            security=True, capabilities=BssidInformationCapabilities()
        )
        neighbor_5g_ap = NeighborReportElement(
            dest_bssid,
            dest_bssid_info,
            operating_class=126,
            channel_number=hostapd_constants.AP_DEFAULT_CHANNEL_5G,
            phy_type=PhyType.VHT,
        )
        btm_req = BssTransitionManagementRequest(
            disassociation_imminent=True,
            candidate_list=BssTransitionCandidateList([neighbor_5g_ap]),
        )

        # Send BTM request from 2.4 GHz AP to DUT
        if self.openwrt_ap:
            self.openwrt_ap.send_bss_transition_management_req(
                client_mac, band=Band.BAND_2G, btm_req=btm_req
            )
        else:
            assert self.access_point is not None
            self.access_point.send_bss_transition_management_req(
                self.access_point.wlan_2g, client_mac, btm_req
            )

        # Check that DUT has not roamed.
        ROAM_DEADLINE = datetime.now(timezone.utc) + timedelta(seconds=2)
        while datetime.now(timezone.utc) < ROAM_DEADLINE:
            # Fail if DUT has reassociated to 5 GHz AP (as seen from AP).
            if self.openwrt_ap:
                try:
                    sta_status = self.openwrt_ap.get_sta_status(
                        client_mac, band=Band.BAND_5G
                    )
                except RuntimeError:
                    sta_status = StationStatus(
                        auth=False, assoc=False, authorized=False
                    )
                if sta_status.authorized:
                    raise signals.TestFailure(
                        "DUT unexpectedly roamed to target BSS after BTM request"
                    )
                else:
                    await asyncio.sleep(0.25)
            elif self.access_point:
                if self.access_point.sta_associated(
                    self.access_point.wlan_5g, client_mac
                ):
                    raise signals.TestFailure(
                        "DUT unexpectedly roamed to target BSS after BTM request"
                    )
                else:
                    await asyncio.sleep(0.25)

        # DUT should have stayed associated to original AP.
        if self.openwrt_ap:
            sta_status = self.openwrt_ap.get_sta_status(
                client_mac, band=Band.BAND_2G
            )
            asserts.assert_true(
                sta_status.assoc,
                "DUT unexpectedly lost association on the 2.4GHz band after BTM request",
            )
        elif self.access_point:
            asserts.assert_true(
                self.access_point.sta_associated(
                    self.access_point.wlan_2g, client_mac
                ),
                "DUT unexpectedly lost association on the 2.4GHz band after BTM request",
            )


if __name__ == "__main__":
    test_runner.main()
