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
from openwrt_access_point.lib.hostapd_options import HostapdOptions
from openwrt_access_point.lib.uci_options import BasicRate, SupportedRates
from openwrt_access_point.lib.uci_radio_options import UciRadioOptions


def netgear_r7000(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Netgear R7000.

    Supported: 2.4GHz and 5GHz, Open or WPA2.
    """
    vendor_elements = (
        "dd0600146c000000"
        "dd310050f204104a00011010440001021047001066189606f1e967f9c0102048817a7"
        "69e103c0001031049000600372a000120"
        "dd1e00904c0408bf0cb259820feaff0000eaff0000c0050001000000c3020002"
        "dd090010180200001c0000"
    )

    n_capabilities = [
        capabilities.N_CAPABILITY_LDPC,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_RX_STBC1,
        capabilities.N_CAPABILITY_MAX_AMSDU_7935,
        capabilities.N_CAPABILITY_SHORT_GI_20,
    ]

    custom_hostapd_options: HostapdOptions = {
        "bss_load_update_period": 50,
        "chan_util_avg_period": 600,
    }

    custom_uci_options: UciRadioOptions = {
        "beacon_int": 100,
    }

    supported_rates = SupportedRates.CCK_AND_OFDM
    if channel.band == Band.BAND_2G:
        basic_rates = BasicRate.CCK_AND_OFDM
        custom_hostapd_options["obss_interval"] = 300
        ac_capabilities = []
    else:
        basic_rates = BasicRate.OFDM_ONLY

        n_capabilities.append(capabilities.N_CAPABILITY_SHORT_GI_40)

        ac_capabilities = [
            capabilities.AC_CAPABILITY_RXLDPC,
            capabilities.AC_CAPABILITY_SHORT_GI_80,
            capabilities.AC_CAPABILITY_TX_STBC_2BY1,
            capabilities.AC_CAPABILITY_RX_STBC_1,
            capabilities.AC_CAPABILITY_MAX_MPDU_11454,
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
                            "dtim_period": 2,
                            "vendor_elements": [vendor_elements],
                            "preamble": False,
                        },
                    )
                ],
                custom_uci_options=custom_uci_options
                | {
                    "supported_rates": supported_rates,
                    "basic_rates": basic_rates,
                },
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
                ac_capabilities=CapabilitySelection.CUSTOM(ac_capabilities),
                custom_hostapd_options=custom_hostapd_options,
            )
        ]
    )


def netgear_wndr3400(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Netgear WNDR3400.

    Supported: 2.4GHz and 5GHz, Open or WPA2.
    """
    vendor_elements = (
        "dd310050f204104a0001101044000102104700108c403eb883e7e225ab139828703ade"
        "dc103c0001031049000600372a000120"
        "dd090010180200f0040000"
    )

    n_capabilities = [
        capabilities.N_CAPABILITY_SHORT_GI_20,
        capabilities.N_CAPABILITY_SHORT_GI_40,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_MAX_AMSDU_7935,
        capabilities.N_CAPABILITY_DSSS_CCK_40,
    ]

    custom_hostapd_options: HostapdOptions = {}
    custom_uci_options: UciRadioOptions = {
        "beacon_int": 100,
    }

    supported_rates = SupportedRates.CCK_AND_OFDM
    if channel.band == Band.BAND_2G:
        basic_rates = BasicRate.CCK_AND_OFDM
        custom_hostapd_options["obss_interval"] = 300
        # DSSS_CCK_40 is duplicated in source, keeping one here.
    else:
        basic_rates = BasicRate.OFDM_ONLY

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
                            "dtim_period": 2,
                            "vendor_elements": [vendor_elements],
                            "preamble": False,
                        },
                    )
                ],
                custom_uci_options=custom_uci_options
                | {
                    "supported_rates": supported_rates,
                    "basic_rates": basic_rates,
                },
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
                custom_hostapd_options=custom_hostapd_options,
            )
        ]
    )


# LINT.ThenChange(//src/testing/end_to_end/antlion/packages/antlion/controllers/ap_lib/third_party_ap_profiles/netgear.py)
