# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests Fuchsia's Wireless Network Management (AKA 802.11v) BSS Transition Management (BTM) support.

Testbed Requirements:
* One Fuchsia device
* One Whirlwind or OpenWrt access point
"""

import asyncio
from dataclasses import dataclass
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
    Security,
    SecurityOpen,
    SecurityWep,
    SecurityWpa,
    SecurityWpa2,
    SecurityWpa3,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)
from openwrt_access_point.lib.extended_capabilities import ExtendedCapabilities
from openwrt_access_point.lib.uci_bss_options import UciBssOptions


@dataclass
class TestParams:
    security: Security


class WlanBssTransitionManagementTest(
    fuchsia_wlan_base_test.FuchsiaWlanBaseTest
):
    """Tests Fuchsia's Wireless Network Management (AKA 802.11v) BTM support.

    Testbed Requirements:
    * One Fuchsia device
    * One Whirlwind or OpenWrt access point
    """

    phy: wlan_core.Phy
    client_iface: wlan_core.ClientIface

    async def pre_run(self) -> None:
        test_args: list[tuple[TestParams]] = []

        SECURITY_MODES: tuple[Security, ...] = (
            SecurityOpen(),
            SecurityWep(),
            SecurityWpa(),
            SecurityWpa2(),
            SecurityWpa3(),
        )
        for security in SECURITY_MODES:
            test_args.append(
                (
                    TestParams(
                        security=security,
                    ),
                )
            )

        def generate_roam_on_btm_req_test_name(test: TestParams) -> str:
            return f"test_roam_on_btm_req_from_{test.security.uci_encryption}_2g_to_{test.security.uci_encryption}_5g"

        self.generate_tests(
            test_logic=self.setup_connect_roam_on_btm_req,
            name_func=generate_roam_on_btm_req_test_name,
            arg_sets=test_args,
        )

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

    def setup_ap(
        self,
        ssid: str,
        security: DeprecatedSecurity | None = None,
        additional_ap_parameters: dict[str, int] | None = None,
        channel: int = hostapd_constants.AP_DEFAULT_CHANNEL_2G,
        wnm_features: FrozenSet[hostapd_constants.WnmFeature] = frozenset(),
    ) -> None:
        """Sets up an AP using the provided parameters."""
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

    async def test_bss_transition_is_advertised_when_ap_supported_dut_supported(
        self,
    ) -> None:
        ssid = AccessPointConfig.random_string(
            hostapd_constants.AP_SSID_LENGTH_2G
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
            wnm_features = frozenset(
                [hostapd_constants.WnmFeature.BSS_TRANSITION_MANAGEMENT]
            )
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
            ext_capabilities = self.get_single_sta_ext_capabilities(
                client_mac, band=Band.BAND_2G
            )
            asserts.assert_true(
                ext_capabilities.bss_transition,
                "DUT is not advertising BSS Transition Management support",
            )
        elif self.access_point:
            legacy_ext_capabilities = (
                self.access_point.get_sta_extended_capabilities(
                    self.access_point.wlan_2g, client_mac
                )
            )
            asserts.assert_true(
                legacy_ext_capabilities.bss_transition,
                "DUT is not advertising BSS Transition Management support",
            )

    # This is called in generate_tests.
    async def setup_connect_roam_on_btm_req(self, test: TestParams) -> None:
        """Setup the APs, associate a DUT, and roam when BTM request is received.

        Args:
            test: Test parameters
        """
        ssid = AccessPointConfig.random_string(
            hostapd_constants.AP_SSID_LENGTH_2G
        )
        password = None
        if not isinstance(test.security, SecurityOpen):
            # Length 13, so it can be used for WEP or WPA
            password = AccessPointConfig.random_string(13)

        wnm_features = frozenset(
            [hostapd_constants.WnmFeature.BSS_TRANSITION_MANAGEMENT]
        )

        # Setup 2.4 GHz AP.
        legacy_security = ConfigMapper.to_hostapd_security(test.security)
        security = DeprecatedSecurity(legacy_security, password)
        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig(
                        channel=DEFAULT_2G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=test.security,
                                password=password,
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
                                security=test.security,
                                password=password,
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
                security=security,
                channel=hostapd_constants.AP_DEFAULT_CHANNEL_2G,
                wnm_features=wnm_features,
            )

        # Match Security to modern SecurityType
        security_type = f_wlan_policy.SecurityType.NONE
        if isinstance(test.security, SecurityWep):
            security_type = f_wlan_policy.SecurityType.WEP
        elif isinstance(test.security, SecurityWpa):
            security_type = f_wlan_policy.SecurityType.WPA
        elif isinstance(test.security, SecurityWpa2):
            security_type = f_wlan_policy.SecurityType.WPA2
        elif isinstance(test.security, SecurityWpa3):
            security_type = f_wlan_policy.SecurityType.WPA3

        await self.dut.wlan_policy.save_network(ssid, security_type, password)
        await self.dut.wlan_policy.connect(ssid, security_type)

        client_mac = await self.client_iface.get_mac_address()

        # Setup 5 GHz AP with same SSID.
        if self.openwrt_ap:
            self.openwrt_ap.enable_radio(Radio.RADIO_5G)
        elif self.access_point:
            self.setup_ap(
                ssid,
                security=security,
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
            operating_class=116,
            channel_number=hostapd_constants.AP_DEFAULT_CHANNEL_5G,
            phy_type=PhyType.VHT,
        )
        btm_req = BssTransitionManagementRequest(
            preferred_candidate_list_included=True,
            disassociation_imminent=True,
            candidate_list=BssTransitionCandidateList([neighbor_5g_ap]),
        )

        # Sleep to avoid concurrent scan during reassociation, necessary due to a firmware bug.
        # TODO(https://fxbug.dev/42068735) Remove when fixed, or when non-firmware BTM support is merged.
        await asyncio.sleep(5)

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

        # Give DUT time to roam.
        ROAM_DEADLINE = datetime.now(timezone.utc) + timedelta(seconds=15)
        while datetime.now(timezone.utc) < ROAM_DEADLINE:
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
                    break
                else:
                    await asyncio.sleep(0.25)
            elif self.access_point:
                if self.access_point.sta_authorized(
                    self.access_point.wlan_5g, client_mac
                ):
                    break
                else:
                    await asyncio.sleep(0.25)

        # Verify that DUT roamed (as seen from AP).
        if self.openwrt_ap:
            sta_status = self.openwrt_ap.get_sta_status(
                client_mac, band=Band.BAND_5G
            )
            asserts.assert_true(
                sta_status.auth, "DUT is not authenticated on the 5GHz band"
            )
            asserts.assert_true(
                sta_status.authorized,
                "DUT is not 802.1X authorized on the 5GHz band",
            )
        elif self.access_point:
            asserts.assert_true(
                self.access_point.sta_authenticated(
                    self.access_point.wlan_5g, client_mac
                ),
                "DUT is not authenticated on the 5GHz band",
            )
            asserts.assert_true(
                self.access_point.sta_authorized(
                    self.access_point.wlan_5g, client_mac
                ),
                "DUT is not 802.1X authorized on the 5GHz band",
            )

    async def test_btm_req_target_ap_rejects_reassoc(self) -> None:
        ssid = AccessPointConfig.random_string(
            hostapd_constants.AP_SSID_LENGTH_2G
        )
        wnm_features = frozenset(
            [hostapd_constants.WnmFeature.BSS_TRANSITION_MANAGEMENT]
        )
        # Setup 2.4 GHz AP.
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

        client_mac = await self.client_iface.get_mac_address()

        # Setup 5 GHz AP with same SSID, but reject all STAs.
        reject_all_sta_param = {"max_num_sta": 0}
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
                                    bss_transition=True,
                                    max_num_sta=0,
                                ),
                            )
                        ],
                    ),
                ]
            )
            self.openwrt_ap.configure_wifi(config)
        elif self.access_point:
            self.setup_ap(
                ssid,
                additional_ap_parameters=reject_all_sta_param,
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
            operating_class=116,
            channel_number=hostapd_constants.AP_DEFAULT_CHANNEL_5G,
            phy_type=PhyType.VHT,
        )
        btm_req = BssTransitionManagementRequest(
            disassociation_imminent=True,
            candidate_list=BssTransitionCandidateList([neighbor_5g_ap]),
        )

        # Sleep to avoid concurrent scan during reassociation, necessary due to a firmware bug.
        # TODO(https://fxbug.dev/42068735) Remove when fixed, or when non-firmware BTM support is merged.
        await asyncio.sleep(5)

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

        # Check that DUT has not reassociated.
        ROAM_DEADLINE = datetime.now(timezone.utc) + timedelta(seconds=2)
        while datetime.now(timezone.utc) < ROAM_DEADLINE:
            # Check that DUT has not reassociated to 5 GHz AP (as seen from AP).
            if self.openwrt_ap:
                try:
                    sta_status = self.openwrt_ap.get_sta_status(
                        client_mac, band=Band.BAND_5G
                    )
                    is_assoc = sta_status.assoc
                except RuntimeError:
                    is_assoc = False
                if is_assoc:
                    raise signals.TestFailure(
                        "DUT unexpectedly roamed to 5GHz band"
                    )
                else:
                    await asyncio.sleep(0.25)
            elif self.access_point:
                if self.access_point.sta_associated(
                    self.access_point.wlan_5g, client_mac
                ):
                    raise signals.TestFailure(
                        "DUT unexpectedly roamed to 5GHz band"
                    )
                else:
                    await asyncio.sleep(0.25)


if __name__ == "__main__":
    test_runner.main()
