#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import itertools
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

import fidl_fuchsia_wlan_internal as fidl_security
import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from antlion.controllers.access_point import setup_ap
from antlion.controllers.ap_lib import hostapd_constants
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.controllers.ap_lib.hostapd_security import (
    SecurityMode as DeprecatedSecurityMode,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point.lib import capabilities
from openwrt_access_point.lib.access_point_config import (
    AccessPointConfig,
    Band,
    BssChannel,
    BssSettings,
    CapabilitySelection,
    RadioConfig,
    SecurityWpa2,
    VhtMode,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)

# AC Capabilities
# TODO(b/505702151): Capabilities Not Supported on Whirlwind. Update test cases to support them with OpenWrt.
"""
Capabilities Not Supported on Whirlwind:
    - Supported Channel Width ([VHT160], [VHT160-80PLUS80]): 160mhz and 80+80
        unsupported
    - SU Beamformer [SU-BEAMFORMER]
    - SU Beamformee [SU-BEAMFORMEE]
    - MU Beamformer [MU-BEAMFORMER]
    - MU Beamformee [MU-BEAMFORMEE]
    - BF Antenna ([BF-ANTENNA-2], [BF-ANTENNA-3], [BF-ANTENNA-4])
    - Rx STBC 2, 3, & 4 ([RX-STBC-12],[RX-STBC-123],[RX-STBC-124])
    - VHT Link Adaptation ([VHT-LINK-ADAPT2],[VHT-LINK-ADAPT3])
    - VHT TXOP Power Save [VHT-TXOP-PS]
    - HTC-VHT [HTC-VHT]
"""

VHT_MAX_MPDU_LEN = [
    capabilities.AC_CAPABILITY_MAX_MPDU_7991,
    capabilities.AC_CAPABILITY_MAX_MPDU_11454,
    "",
]
RXLDPC = [capabilities.AC_CAPABILITY_RXLDPC, ""]
SHORT_GI_80 = [capabilities.AC_CAPABILITY_SHORT_GI_80, ""]
TX_STBC = [capabilities.AC_CAPABILITY_TX_STBC_2BY1, ""]
RX_STBC = [capabilities.AC_CAPABILITY_RX_STBC_1, ""]
MAX_A_MPDU = [
    capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP0,
    capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP1,
    capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP2,
    capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP3,
    capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP4,
    capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP5,
    capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP6,
    capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP7,
    "",
]
RX_ANTENNA = [capabilities.AC_CAPABILITY_RX_ANTENNA_PATTERN, ""]
TX_ANTENNA = [capabilities.AC_CAPABILITY_TX_ANTENNA_PATTERN, ""]

# Default 11N Capabilities
N_CAPABS_40MHZ = [
    capabilities.N_CAPABILITY_LDPC,
    capabilities.N_CAPABILITY_SHORT_GI_20,
    capabilities.N_CAPABILITY_RX_STBC1,
    capabilities.N_CAPABILITY_SHORT_GI_20,
    capabilities.N_CAPABILITY_SHORT_GI_40,
    capabilities.N_CAPABILITY_MAX_AMSDU_7935,
    capabilities.N_CAPABILITY_HT40_PLUS,
]

N_CAPABS_20MHZ = [
    capabilities.N_CAPABILITY_LDPC,
    capabilities.N_CAPABILITY_SHORT_GI_20,
    capabilities.N_CAPABILITY_RX_STBC1,
    capabilities.N_CAPABILITY_SHORT_GI_20,
    capabilities.N_CAPABILITY_MAX_AMSDU_7935,
    capabilities.N_CAPABILITY_HT20,
]


@dataclass
class TestParams:
    vht_bandwidth_mhz: Literal[20, 40, 80, 160]
    # TODO(http://b/290396383): Type AP capabilities as enums
    n_capabilities: list[str]
    ac_capabilities: list[str]


# 3456 test cases
class WlanPhyCompliance11ACTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """Tests for validating 11ac PHYS.

    Test Bed Requirement:
    * One Fuchsia device
    * One Access Point
    """

    phy: wlan_core.Phy

    async def setup_class(self) -> None:
        await super().setup_class()
        if self.access_point:
            self.access_point.stop_all_aps()
        self.phy = await self.dut.wlan_core.ensure_single_phy()

        # 802.11ac only specifies operation in the 5 GHz band.
        # Set country to US so that 5 GHz channels are supported.
        await self.phy.set_country(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_core.destroy_all_ifaces()

    async def teardown_test(self) -> None:
        await self.dut.wlan_core.destroy_all_ifaces()
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    async def pre_run(self) -> None:
        test_args: list[tuple[TestParams]] = (
            self._generate_20mhz_test_args()
            + self._generate_40mhz_test_args()
            + self._generate_80mhz_test_args()
        )

        def generate_test_name(params: TestParams) -> str:
            ret = []
            mapped_caps = [
                ConfigMapper.to_hostapd_ac_cap(c)
                for c in params.ac_capabilities
                if c
            ]
            for cap in hostapd_constants.AC_CAPABILITIES_MAPPING.keys():
                if cap in mapped_caps:
                    ret.append(
                        hostapd_constants.AC_CAPABILITIES_MAPPING[cap]
                        .replace("[", "_")
                        .replace("]", "")
                    )

            # Maintain legacy naming for BUILD.gn filters
            return f"test_11ac_{params.vht_bandwidth_mhz}mhz_wpa2{''.join(ret)}"

        self.generate_tests(
            test_logic=self.setup_and_connect,
            name_func=generate_test_name,
            arg_sets=test_args,
        )

    async def setup_and_connect(self, params: TestParams) -> None:
        """Setup the AP and then attempt to associate a DUT.

        Args:
            params: Test parameters
        """
        ssid = AccessPointConfig.random_string(
            hostapd_constants.AP_SSID_LENGTH_2G
        )
        password = AccessPointConfig.random_string()
        security = DeprecatedSecurity(
            security_mode=DeprecatedSecurityMode.WPA2,
            password=password,
            wpa_cipher=hostapd_constants.WPA2_DEFAULT_CIPER,
            wpa2_cipher=hostapd_constants.WPA2_DEFAULT_CIPER,
        )
        protocol = fidl_security.Protocol.WPA2_PERSONAL

        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=BssChannel(
                            band=Band.BAND_5G,
                            number=36,
                            phy_mode=VhtMode(bw=params.vht_bandwidth_mhz),
                        ),
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=SecurityWpa2(),
                                password=password,
                            )
                        ],
                        n_capabilities=CapabilitySelection.CUSTOM(
                            params.n_capabilities
                        ),
                        ac_capabilities=CapabilitySelection.CUSTOM(
                            params.ac_capabilities
                        ),
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
            await self._connect_and_validate_channel(
                ssid,
                protocol,
                target_channel=36,
                target_pwd=password,
            )
        elif self.access_point:
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                mode=hostapd_constants.Mode.MODE_11AC_MIXED,
                channel=36,
                n_capabilities=[
                    ConfigMapper.to_hostapd_n_cap(c)
                    for c in params.n_capabilities
                    if c
                ],
                ac_capabilities=[
                    ConfigMapper.to_hostapd_ac_cap(c)
                    for c in params.ac_capabilities
                    if c
                ],
                force_wmm=True,
                ssid=ssid,
                security=security,
                vht_bandwidth=params.vht_bandwidth_mhz,
            )
            with self.access_point.tcpdump.start(
                self.access_point.wlan_5g, Path(self.log_path)
            ):
                await self._connect_and_validate_channel(
                    ssid,
                    protocol,
                    target_channel=36,
                    target_pwd=password,
                )

    async def _connect_and_validate_channel(
        self,
        target_ssid: str,
        target_security: fidl_security.Protocol,
        target_channel: int,
        target_pwd: str | None = None,
    ) -> None:
        iface = await self.phy.create_client_iface()
        await iface.scan_and_connect(
            ssid=target_ssid,
            password=target_pwd,
            security=target_security,
        )
        status = await iface.status()
        if status.connected is None:
            raise signals.TestFailure(
                f"Expected connected status, got: {status}"
            )
        got_channel = status.connected.primary.number
        asserts.assert_equal(
            got_channel,
            target_channel,
            f"Connected to wrong channel. Expected channel {target_channel}, "
            f"got {got_channel}.",
        )

    # 864 tests
    def _generate_20mhz_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []

        for combination in itertools.product(
            VHT_MAX_MPDU_LEN,
            RXLDPC,
            RX_STBC,
            TX_STBC,
            MAX_A_MPDU,
            RX_ANTENNA,
            TX_ANTENNA,
        ):
            test_args.append(
                (
                    TestParams(
                        vht_bandwidth_mhz=20,
                        n_capabilities=N_CAPABS_20MHZ,
                        ac_capabilities=list(combination),
                    ),
                )
            )

        return test_args

    # 864 tests
    def _generate_40mhz_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []

        for combination in itertools.product(
            VHT_MAX_MPDU_LEN,
            RXLDPC,
            RX_STBC,
            TX_STBC,
            MAX_A_MPDU,
            RX_ANTENNA,
            TX_ANTENNA,
        ):
            test_args.append(
                (
                    TestParams(
                        vht_bandwidth_mhz=40,
                        n_capabilities=N_CAPABS_40MHZ,
                        ac_capabilities=list(combination),
                    ),
                )
            )

        return test_args

    # 1728 tests
    def _generate_80mhz_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []

        for combination in itertools.product(
            VHT_MAX_MPDU_LEN,
            RXLDPC,
            SHORT_GI_80,
            RX_STBC,
            TX_STBC,
            MAX_A_MPDU,
            RX_ANTENNA,
            TX_ANTENNA,
        ):
            test_args.append(
                (
                    TestParams(
                        vht_bandwidth_mhz=80,
                        n_capabilities=N_CAPABS_40MHZ,
                        ac_capabilities=list(combination),
                    ),
                )
            )
        return test_args


if __name__ == "__main__":
    test_runner.main()
