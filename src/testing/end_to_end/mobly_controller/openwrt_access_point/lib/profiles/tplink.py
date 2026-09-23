# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# LINT.IfChange

from openwrt_access_point.lib import capabilities
from openwrt_access_point.lib.access_point_config import (
    AccessPointConfig,
    Band,
    BssChannel,
    BssSettings,
    CapabilitySelection,
    RadioConfig,
    Security,
)
from openwrt_access_point.lib.uci_options import BasicRate, SupportedRates
from openwrt_access_point.lib.uci_radio_options import UciRadioOptions


def tplink_archerc5(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for TPLink ArcherC5."""
    vendor_elements = (
        "dd310050f204104a000110104400010210470010d96c7efc2f8938f1efbd6e5148bfa8"
        "12103c0001031049000600372a000120"
        "dd090010180200001c0000"
    )

    n_capabilities = [
        capabilities.N_CAPABILITY_SHORT_GI_20,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_RX_STBC1,
        capabilities.N_CAPABILITY_MAX_AMSDU_7935,
    ]

    if channel.band == Band.BAND_2G:
        n_capabilities.append(capabilities.N_CAPABILITY_DSSS_CCK_40)
        ac_capabilities = []
        supported_rates = SupportedRates.CCK_AND_OFDM
        basic_rates = BasicRate.CCK_AND_OFDM
        preamble = True
    else:
        n_capabilities.append(capabilities.N_CAPABILITY_LDPC)
        ac_capabilities = [
            capabilities.AC_CAPABILITY_MAX_MPDU_11454,
            capabilities.AC_CAPABILITY_SHORT_GI_80,
            capabilities.AC_CAPABILITY_RXLDPC,
            capabilities.AC_CAPABILITY_TX_STBC_2BY1,
            capabilities.AC_CAPABILITY_RX_STBC_1,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP7,
        ]
        supported_rates = SupportedRates.OFDM
        basic_rates = BasicRate.OFDM_ONLY
        preamble = False

    return AccessPointConfig(
        radios=[
            RadioConfig(
                channel=channel,
                bss_settings=[
                    BssSettings(
                        ssid=ssid,
                        security=security,
                        password=password,
                        custom_uci_options={
                            "dtim_period": 1,
                            "vendor_elements": [vendor_elements],
                            "preamble": preamble,
                            "bss_load_update_period": 50,
                            "chan_util_avg_period": 600,
                            "rrm_beacon_report": True,
                            "rrm_neighbor_report": True,
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": supported_rates,
                    "basic_rates": basic_rates,
                },
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
                ac_capabilities=CapabilitySelection.CUSTOM(ac_capabilities),
            )
        ]
    )


def tplink_archerc7(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for TPLink ArcherC7."""
    vendor_elements = (
        "dd0900037f01010000ff7f"
        "dd180050f204104a00011010440001021049000600372a000120"
    )

    n_capabilities = [
        capabilities.N_CAPABILITY_LDPC,
        capabilities.N_CAPABILITY_SHORT_GI_20,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_RX_STBC1,
    ]

    if channel.band == Band.BAND_2G:
        supported_rates = SupportedRates.CCK_AND_OFDM
        basic_rates = BasicRate.CCK_AND_OFDM
        preamble = True
        ac_capabilities = []
    else:
        supported_rates = SupportedRates.OFDM
        basic_rates = BasicRate.OFDM_ONLY
        preamble = False
        n_capabilities.extend(
            [
                capabilities.N_CAPABILITY_SHORT_GI_40,
                capabilities.N_CAPABILITY_MAX_AMSDU_7935,
            ]
        )
        # Add HT40+ if channel is 36 (typical default)
        if channel.number == 36:
            n_capabilities.append(capabilities.N_CAPABILITY_HT40_PLUS)

        ac_capabilities = [
            capabilities.AC_CAPABILITY_MAX_MPDU_11454,
            capabilities.AC_CAPABILITY_RXLDPC,
            capabilities.AC_CAPABILITY_SHORT_GI_80,
            capabilities.AC_CAPABILITY_TX_STBC_2BY1,
            capabilities.AC_CAPABILITY_RX_STBC_1,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP7,
            capabilities.AC_CAPABILITY_RX_ANTENNA_PATTERN,
            capabilities.AC_CAPABILITY_TX_ANTENNA_PATTERN,
        ]
        vendor_elements += (
            "074255532024011e28011e2c011e30"
            "011e3401173801173c01174001176401176801176c0117700117740117840117"
            "8801178c011795011e99011e9d011ea1011ea5011e"
        )
    radio_uci_options: UciRadioOptions = {
        "beacon_int": 100,
        "supported_rates": supported_rates,
        "basic_rates": basic_rates,
    }
    if channel.band == Band.BAND_5G:
        radio_uci_options["local_pwr_constraint"] = 3

    return AccessPointConfig(
        radios=[
            RadioConfig(
                channel=channel,
                bss_settings=[
                    BssSettings(
                        ssid=ssid,
                        security=security,
                        password=password,
                        custom_uci_options={
                            "dtim_period": 1,
                            "vendor_elements": [vendor_elements],
                            "preamble": preamble,
                        },
                    )
                ],
                custom_uci_options=radio_uci_options,
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
                ac_capabilities=CapabilitySelection.CUSTOM(ac_capabilities),
            )
        ]
    )


def tplink_c1200(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for TPLink C1200."""
    vendor_elements = (
        "dd350050f204104a000110104400010210470010000000000000000000000000000000"
        "00103c0001031049000a00372a00012005022688"
        "dd090010180200000c0000"
    )

    n_capabilities = [
        capabilities.N_CAPABILITY_SHORT_GI_20,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_RX_STBC1,
        capabilities.N_CAPABILITY_MAX_AMSDU_7935,
    ]

    if channel.band == Band.BAND_2G:
        supported_rates = SupportedRates.CCK_AND_OFDM
        basic_rates = BasicRate.CCK_AND_OFDM
        preamble = True
        ac_capabilities = []
    else:
        supported_rates = SupportedRates.OFDM
        basic_rates = BasicRate.OFDM_ONLY
        preamble = False
        n_capabilities.append(capabilities.N_CAPABILITY_LDPC)
        ac_capabilities = [
            capabilities.AC_CAPABILITY_MAX_MPDU_11454,
            capabilities.AC_CAPABILITY_SHORT_GI_80,
            capabilities.AC_CAPABILITY_RXLDPC,
            capabilities.AC_CAPABILITY_TX_STBC_2BY1,
            capabilities.AC_CAPABILITY_RX_STBC_1,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP7,
        ]

    return AccessPointConfig(
        radios=[
            RadioConfig(
                channel=channel,
                bss_settings=[
                    BssSettings(
                        ssid=ssid,
                        security=security,
                        password=password,
                        custom_uci_options={
                            "dtim_period": 1,
                            "vendor_elements": [vendor_elements],
                            "preamble": preamble,
                            "rrm_beacon_report": True,
                            "rrm_neighbor_report": True,
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": supported_rates,
                    "basic_rates": basic_rates,
                },
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
                ac_capabilities=CapabilitySelection.CUSTOM(ac_capabilities),
            )
        ]
    )


def tplink_tlwr940n(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for TPLink TLWR940N."""
    if channel.band != Band.BAND_2G:
        raise ValueError("TPLink TLWR940N only supports 2.4GHz")

    vendor_elements = (
        "dd0900037f01010000ff7f"
        "dd260050f204104a0001101044000102104900140024e2600200010160000002000160"
        "0100020001"
    )

    n_capabilities = [
        capabilities.N_CAPABILITY_SHORT_GI_20,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_RX_STBC1,
    ]

    return AccessPointConfig(
        radios=[
            RadioConfig(
                channel=channel,
                bss_settings=[
                    BssSettings(
                        ssid=ssid,
                        security=security,
                        password=password,
                        custom_uci_options={
                            "dtim_period": 1,
                            "vendor_elements": [vendor_elements],
                            "preamble": True,
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": SupportedRates.CCK_AND_OFDM,
                    "basic_rates": BasicRate.CCK_AND_OFDM,
                },
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
            )
        ]
    )


# LINT.ThenChange(//src/testing/end_to_end/antlion/packages/antlion/controllers/ap_lib/third_party_ap_profiles/tplink.py)
