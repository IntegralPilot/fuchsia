#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import itertools
from dataclasses import dataclass
from typing import Any, Literal

import fuchsia_async_extension
import honeydew.affordances.connectivity.wlan.core as wlan_core
from antlion.controllers.access_point import AccessPoint, setup_ap
from antlion.controllers.ap_lib import hostapd_config, hostapd_constants
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.controllers.ap_lib.hostapd_security import (
    SecurityMode as DeprecatedSecurityMode,
)
from fuchsia_wlan_base_test.deprecated.wifi import base_test
from mobly import asserts, signals, test_runner
from mobly.config_parser import TestRunConfig
from mobly.records import TestResultRecord
from openwrt_access_point.lib import capabilities
from openwrt_access_point.lib.access_point_config import (
    AccessPointConfig,
    Band,
    BssChannel,
    BssSettings,
    CapabilitySelection,
    HtMode,
    RadioConfig,
    Security,
    SecurityOpen,
    SecurityWpa2,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)
from openwrt_access_point.lib.uci_radio_options import UciRadioOptions

FREQUENCY_24: str = "2.4GHz"
FREQUENCY_5: str = "5GHz"
CHANNEL_BANDWIDTH_20: str = "HT20"
CHANNEL_BANDWIDTH_40_LOWER: str = "HT40-"
CHANNEL_BANDWIDTH_40_UPPER: str = "HT40+"
SECURITY_OPEN = "open"
SECURITY_WPA2 = "wpa2"
N_MODE = [
    hostapd_constants.Mode.MODE_11N_PURE,
    hostapd_constants.Mode.MODE_11N_MIXED,
]
LDPC = [capabilities.N_CAPABILITY_LDPC, ""]
TX_STBC = [capabilities.N_CAPABILITY_TX_STBC, ""]
RX_STBC = [capabilities.N_CAPABILITY_RX_STBC1, ""]
SGI_20 = [capabilities.N_CAPABILITY_SHORT_GI_20, ""]
SGI_40 = [capabilities.N_CAPABILITY_SHORT_GI_40, ""]
DSSS_CCK = [capabilities.N_CAPABILITY_DSSS_CCK_40, ""]
INTOLERANT_40 = [capabilities.N_CAPABILITY_40_INTOLERANT, ""]
MAX_AMPDU_7935 = [capabilities.N_CAPABILITY_MAX_AMSDU_7935, ""]
SMPS = [capabilities.N_CAPABILITY_SMPS_STATIC, ""]

KNOWN_UNSUPPORTED_CAPABILITIES = {
    capabilities.N_CAPABILITY_40_INTOLERANT,
    capabilities.N_CAPABILITY_SMPS_STATIC,
}


@dataclass
class TestParams:
    frequency: str
    chbw: str
    n_mode: str
    security: Security
    # TODO(http://b/290396383): Type AP capabilities as enums
    n_capabilities: list[Any]


class WlanPhyCompliance11NTest(base_test.WifiBaseTest):
    """Tests for validating 11n PHYS.

    Test Bed Requirement:
    * One Android device or Fuchsia device
    * One Access Point
    """

    access_point: AccessPoint | None = None
    openwrt_ap: Any | None = None

    async def _get_client_iface(self) -> wlan_core.ClientIface:
        phy = await self.dut.device.honeydew_fd.wlan_core.ensure_single_phy()
        client_ifaces = await phy.get_client_ifaces()
        if client_ifaces:
            return client_ifaces[0]
        return await phy.create_client_iface()

    def _connect_and_validate_channel(
        self,
        target_ssid: str,
        target_security: DeprecatedSecurityMode,
        target_channel: int,
        target_pwd: str | None = None,
    ) -> None:
        async def _connect_and_validate() -> None:
            await self.dut.device.honeydew_fd.wlan_policy.save_network(
                target_ssid,
                target_security.fuchsia_security_type(),
                target_pwd=target_pwd,
            )
            await self.dut.device.honeydew_fd.wlan_policy.connect(
                target_ssid,
                target_security.fuchsia_security_type(),
            )
            iface = await self._get_client_iface()
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

        fuchsia_async_extension.get_loop().run_until_complete(
            _connect_and_validate()
        )

    def __init__(self, config: TestRunConfig) -> None:
        super().__init__(config)

    def pre_run(self) -> None:
        test_args: list[tuple[TestParams]] = (
            self._generate_24_HT20_test_args()
            + self._generate_24_HT40_lower_test_args()
            + self._generate_24_HT40_upper_test_args()
            + self._generate_5_HT20_test_args()
            + self._generate_5_HT40_lower_test_args()
            + self._generate_5_HT40_upper_test_args()
            + self._generate_24_HT20_wpa2_test_args()
            + self._generate_24_HT40_lower_wpa2_test_args()
            + self._generate_24_HT40_upper_wpa2_test_args()
            + self._generate_5_HT20_wpa2_test_args()
            + self._generate_5_HT40_lower_wpa2_test_args()
            + self._generate_5_HT40_upper_wpa2_test_args()
        )

        def generate_test_name(test: TestParams) -> str:
            ret = []
            mapped_caps = [
                ConfigMapper.to_hostapd_n_cap(c)
                for c in test.n_capabilities
                if c
            ]
            for cap in hostapd_constants.N_CAPABILITIES_MAPPING.keys():
                if cap in mapped_caps:
                    ret.append(
                        hostapd_constants.N_CAPABILITIES_MAPPING[cap]
                        .replace("[", "_")
                        .replace("]", "")
                    )
            # '+' is used by Mobile Harness as special character, don't use it in test names
            if test.chbw == "HT40-":
                chbw = "HT40Lower"
            elif test.chbw == "HT40+":
                chbw = "HT40Upper"
            else:
                chbw = test.chbw
            # Maintain legacy naming for BUILD.gn filters
            security_name = (
                "open" if test.security == SecurityOpen() else "wpa2"
            )
            return f"test_11n_{test.frequency}_{chbw}_{security_name}_{test.n_mode}{''.join(ret)}"

        self.generate_tests(
            test_logic=self.setup_and_connect,
            name_func=generate_test_name,
            arg_sets=test_args,
        )

    def setup_class(self) -> None:
        super().setup_class()

        if self.openwrt_aps:
            self.openwrt_ap = self.openwrt_aps[0]
        elif self.access_points:
            self.access_point = self.access_points[0]
        else:
            raise signals.TestAbortClass(
                "At least one access point is required"
            )

        self.dut = self.get_dut()

        if self.access_point:
            self.access_point.stop_all_aps()

    def teardown_test(self) -> None:
        self.dut.disconnect()
        self.download_logs()
        if self.access_point:
            self.access_point.stop_all_aps()

    def on_fail(self, record: TestResultRecord) -> None:
        super().on_fail(record)
        if self.access_point:
            self.access_point.stop_all_aps()

    def setup_and_connect(self, test: TestParams) -> None:
        """Start hostapd and associate the DUT.

        Args:
               test: Test parameters
        """
        ssid = AccessPointConfig.random_string(20)
        password: str | None = None

        if test.chbw == "HT20" or test.chbw == "HT40+":
            if test.frequency == "2.4GHz":
                channel = 1
            elif test.frequency == "5GHz":
                channel = 36
            else:
                raise ValueError(f"Invalid frequency: {test.frequency}")

        elif test.chbw == "HT40-":
            if test.frequency == "2.4GHz":
                channel = 11
            elif test.frequency == "5GHz":
                channel = 60
            else:
                raise ValueError(f"Invalid frequency: {test.frequency}")
        else:
            raise ValueError(f"Invalid channel bandwidth: {test.chbw}")

        if test.security == SecurityWpa2():
            password = AccessPointConfig.random_string(20)

        if self.openwrt_ap:
            band = Band.BAND_2G if test.frequency == "2.4GHz" else Band.BAND_5G
            bandwidth: Literal[20, 40] = 40 if "HT40" in test.chbw else 20

            extension_channel: Literal["+", "-", None] = None
            if test.chbw == CHANNEL_BANDWIDTH_40_UPPER:
                extension_channel = "+"
            elif test.chbw == CHANNEL_BANDWIDTH_40_LOWER:
                extension_channel = "-"

            for cap in test.n_capabilities:
                if cap in KNOWN_UNSUPPORTED_CAPABILITIES:
                    raise signals.TestSkip(
                        f"Skipping test because capability '{cap}' is unsupported on OpenWrt"
                    )

            n_caps = [cap for cap in test.n_capabilities if cap]

            require_mode: Literal["n", "ac", "ax"] | None = None
            if test.n_mode == hostapd_constants.Mode.MODE_11N_PURE.value:
                require_mode = "n"
            elif test.n_mode == hostapd_constants.Mode.MODE_11AC_PURE.value:
                require_mode = "ac"

            custom_uci_options: UciRadioOptions = {}
            if require_mode:
                custom_uci_options["require_mode"] = require_mode

            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=BssChannel(
                            band=band,
                            number=channel,
                            phy_mode=HtMode(
                                bw=bandwidth, extension=extension_channel
                            ),
                        ),
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=test.security,
                                password=password,
                            )
                        ],
                        n_capabilities=CapabilitySelection.CUSTOM(n_caps),
                        custom_uci_options=custom_uci_options,
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)

            self._connect_and_validate_channel(
                ssid,
                ConfigMapper.to_hostapd_security(test.security),
                target_channel=channel,
                target_pwd=password,
            )
        elif self.access_point:
            security_profile = DeprecatedSecurity()
            n_capabilities = []
            for cap in test.n_capabilities:
                if not cap:
                    continue
                mapped_cap = ConfigMapper.to_hostapd_n_cap(cap)
                if (
                    mapped_cap
                    in hostapd_constants.N_CAPABILITIES_MAPPING.keys()
                ):
                    n_capabilities.append(mapped_cap)

            if test.chbw == "HT40-" or test.chbw == "HT40+":
                if hostapd_config.ht40_plus_allowed(channel):
                    extended_channel = hostapd_constants.N_CAPABILITY_HT40_PLUS
                elif hostapd_config.ht40_minus_allowed(channel):
                    extended_channel = hostapd_constants.N_CAPABILITY_HT40_MINUS
                else:
                    raise ValueError(f"Invalid channel: {channel}")
                n_capabilities.append(extended_channel)

            if test.security == SecurityWpa2():
                security_profile = DeprecatedSecurity(
                    security_mode=DeprecatedSecurityMode.WPA2,
                    password=password,
                    wpa_cipher="CCMP",
                    wpa2_cipher="CCMP",
                )

            if test.n_mode not in N_MODE:
                raise ValueError(f"Invalid n-mode: {test.n_mode}")

            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                mode=test.n_mode,
                channel=channel,
                n_capabilities=n_capabilities,
                ac_capabilities=[],
                force_wmm=True,
                ssid=ssid,
                security=security_profile,
            )
            self._connect_and_validate_channel(
                ssid,
                ConfigMapper.to_hostapd_security(test.security),
                target_channel=channel,
                target_pwd=password,
            )

    def _generate_24_HT20_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            N_MODE,
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            INTOLERANT_40,
            MAX_AMPDU_7935,
            SMPS,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_24,
                        chbw=CHANNEL_BANDWIDTH_20,
                        n_mode=combination[0],
                        security=SecurityOpen(),
                        n_capabilities=list(combination[1:]),
                    ),
                )
            )
        return test_args

    def _generate_24_HT40_lower_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            SGI_40,
            MAX_AMPDU_7935,
            SMPS,
            DSSS_CCK,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_24,
                        chbw=CHANNEL_BANDWIDTH_40_LOWER,
                        n_mode=hostapd_constants.Mode.MODE_11N_MIXED,
                        security=SecurityOpen(),
                        n_capabilities=list(combination),
                    ),
                )
            )
        return test_args

    def _generate_24_HT40_upper_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            SGI_40,
            MAX_AMPDU_7935,
            SMPS,
            DSSS_CCK,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_24,
                        chbw=CHANNEL_BANDWIDTH_40_UPPER,
                        n_mode=hostapd_constants.Mode.MODE_11N_MIXED,
                        security=SecurityOpen(),
                        n_capabilities=list(combination),
                    ),
                )
            )
        return test_args

    def _generate_5_HT20_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            INTOLERANT_40,
            MAX_AMPDU_7935,
            SMPS,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_5,
                        chbw=CHANNEL_BANDWIDTH_20,
                        n_mode=hostapd_constants.Mode.MODE_11N_MIXED,
                        security=SecurityOpen(),
                        n_capabilities=list(combination),
                    ),
                )
            )
        return test_args

    def _generate_5_HT40_lower_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            SGI_40,
            MAX_AMPDU_7935,
            SMPS,
            DSSS_CCK,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_5,
                        chbw=CHANNEL_BANDWIDTH_40_LOWER,
                        n_mode=hostapd_constants.Mode.MODE_11N_MIXED,
                        security=SecurityOpen(),
                        n_capabilities=list(combination),
                    ),
                )
            )
        return test_args

    def _generate_5_HT40_upper_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            N_MODE,
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            SGI_40,
            MAX_AMPDU_7935,
            SMPS,
            DSSS_CCK,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_5,
                        chbw=CHANNEL_BANDWIDTH_40_UPPER,
                        n_mode=combination[0],
                        security=SecurityOpen(),
                        n_capabilities=list(combination[1:]),
                    ),
                )
            )
        return test_args

    def _generate_24_HT20_wpa2_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            INTOLERANT_40,
            MAX_AMPDU_7935,
            SMPS,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_24,
                        chbw=CHANNEL_BANDWIDTH_20,
                        n_mode=hostapd_constants.Mode.MODE_11N_MIXED,
                        security=SecurityWpa2(),
                        n_capabilities=list(combination),
                    ),
                )
            )
        return test_args

    def _generate_24_HT40_lower_wpa2_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            SGI_40,
            MAX_AMPDU_7935,
            SMPS,
            DSSS_CCK,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_24,
                        chbw=CHANNEL_BANDWIDTH_40_LOWER,
                        n_mode=hostapd_constants.Mode.MODE_11N_MIXED,
                        security=SecurityWpa2(),
                        n_capabilities=list(combination),
                    ),
                )
            )
        return test_args

    def _generate_24_HT40_upper_wpa2_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            SGI_40,
            MAX_AMPDU_7935,
            SMPS,
            DSSS_CCK,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_24,
                        chbw=CHANNEL_BANDWIDTH_40_UPPER,
                        n_mode=hostapd_constants.Mode.MODE_11N_MIXED,
                        security=SecurityWpa2(),
                        n_capabilities=list(combination),
                    ),
                )
            )
        return test_args

    def _generate_5_HT20_wpa2_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            INTOLERANT_40,
            MAX_AMPDU_7935,
            SMPS,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_5,
                        chbw=CHANNEL_BANDWIDTH_20,
                        n_mode=hostapd_constants.Mode.MODE_11N_MIXED,
                        security=SecurityWpa2(),
                        n_capabilities=list(combination),
                    ),
                )
            )
        return test_args

    def _generate_5_HT40_lower_wpa2_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            SGI_40,
            MAX_AMPDU_7935,
            SMPS,
            DSSS_CCK,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_5,
                        chbw=CHANNEL_BANDWIDTH_40_LOWER,
                        n_mode=hostapd_constants.Mode.MODE_11N_MIXED,
                        security=SecurityWpa2(),
                        n_capabilities=list(combination),
                    ),
                )
            )
        return test_args

    def _generate_5_HT40_upper_wpa2_test_args(self) -> list[tuple[TestParams]]:
        test_args: list[tuple[TestParams]] = []
        for combination in itertools.product(
            LDPC,
            TX_STBC,
            RX_STBC,
            SGI_20,
            SGI_40,
            MAX_AMPDU_7935,
            SMPS,
            DSSS_CCK,
        ):
            test_args.append(
                (
                    TestParams(
                        frequency=FREQUENCY_5,
                        chbw=CHANNEL_BANDWIDTH_40_UPPER,
                        n_mode=hostapd_constants.Mode.MODE_11N_MIXED,
                        security=SecurityWpa2(),
                        n_capabilities=list(combination),
                    ),
                )
            )
        return test_args


if __name__ == "__main__":
    test_runner.main()
