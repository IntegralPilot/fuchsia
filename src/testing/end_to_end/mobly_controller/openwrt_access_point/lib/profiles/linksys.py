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


def linksys_ea4500(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Linksys EA4500.

    Supported: 2.4GHz and 5GHz, Open or WPA2.
    """
    vendor_elements = (
        "dd1e00904c33fc0117ffffff0000000000000000000000000000000000000000"
        "dd1a00904c3424000000000000000000000000000000000000000000"
        "dd06005043030000"
    )

    n_capabilities = [
        capabilities.N_CAPABILITY_SHORT_GI_20,
        capabilities.N_CAPABILITY_SHORT_GI_40,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_RX_STBC1,
        capabilities.N_CAPABILITY_DSSS_CCK_40,
    ]

    custom_hostapd_options: HostapdOptions = {}
    supported_rates = SupportedRates.CCK_AND_OFDM
    if channel.band == Band.BAND_2G:
        basic_rates = BasicRate.CCK_AND_OFDM
        custom_hostapd_options["obss_interval"] = 180
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
                            "dtim_period": 1,
                            "vendor_elements": [vendor_elements],
                            "preamble": True,
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": supported_rates,
                    "basic_rates": basic_rates,
                },
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
                custom_hostapd_options=custom_hostapd_options,
            )
        ]
    )


def linksys_ea9500(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Linksys EA9500.

    Supported: 2.4GHz and 5GHz, Open or WPA2.
    """
    vendor_elements = "42020000"

    supported_rates = SupportedRates.CCK_AND_OFDM
    if channel.band == Band.BAND_2G:
        basic_rates = BasicRate.CCK_AND_OFDM
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
                            "dtim_period": 1,
                            "vendor_elements": [vendor_elements],
                            "preamble": False,
                            "bss_load_update_period": 50,
                            "chan_util_avg_period": 600,
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": supported_rates,
                    "basic_rates": basic_rates,
                },
            )
        ]
    )


def linksys_wrt1900acv2(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Linksys WRT1900ACV2.

    Supported: 2.4GHz and 5GHz, Open or WPA2.
    """
    vendor_elements = (
        "dd1e00904c336c0017ffffff0001000000000000000000000000001fff071800"
        "dd1a00904c3424000000000000000000000000000000000000000000"
        "dd06005043030000"
    )

    n_capabilities = [
        capabilities.N_CAPABILITY_LDPC,
        capabilities.N_CAPABILITY_SHORT_GI_20,
        capabilities.N_CAPABILITY_SHORT_GI_40,
    ]

    ac_capabilities = [
        capabilities.AC_CAPABILITY_RXLDPC,
        capabilities.AC_CAPABILITY_SHORT_GI_80,
        capabilities.AC_CAPABILITY_RX_STBC_1,
        capabilities.AC_CAPABILITY_RX_ANTENNA_PATTERN,
        capabilities.AC_CAPABILITY_TX_ANTENNA_PATTERN,
        capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP7,
    ]

    custom_uci_options: UciRadioOptions = {
        "beacon_int": 100,
        "supported_rates": SupportedRates.CCK_AND_OFDM,
    }

    custom_hostapd_options: HostapdOptions = {}
    if channel.band == Band.BAND_2G:
        custom_uci_options["basic_rates"] = BasicRate.CCK_AND_OFDM
        custom_hostapd_options["obss_interval"] = 180
    else:
        custom_uci_options["basic_rates"] = BasicRate.OFDM_ONLY
        custom_uci_options["ieee80211h"] = True
        custom_uci_options["ieee80211d"] = True
        custom_uci_options["spectrum_mgmt_required"] = True
        custom_uci_options["local_pwr_constraint"] = 3
        vendor_elements += (
            "071e5553202401112801112c011130"
            "01119501179901179d0117a10117a50117"
        )

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
                custom_uci_options=custom_uci_options,
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
                ac_capabilities=CapabilitySelection.CUSTOM(ac_capabilities),
                custom_hostapd_options=custom_hostapd_options,
            )
        ]
    )


# LINT.ThenChange(//src/testing/end_to_end/antlion/packages/antlion/controllers/ap_lib/third_party_ap_profiles/linksys.py)
