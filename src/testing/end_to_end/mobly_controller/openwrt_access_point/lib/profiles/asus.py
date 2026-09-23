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
    HtMode,
    LegacyMode,
    RadioConfig,
    Security,
    VhtMode,
)
from openwrt_access_point.lib.hostapd_options import HostapdOptions
from openwrt_access_point.lib.uci_options import BasicRate, SupportedRates
from openwrt_access_point.lib.uci_radio_options import UciRadioOptions


def asus_rtac66u(
    channel: int,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Asus RTAC66U.

    Supported: 2.4GHz and 5GHz, Open or WPA2.
    """

    vendor_elements = (
        "dd310050f204104a00011010440001021047001093689729d373c26cb1563c6c570f33"
        "d7103c0001031049000600372a000120"
        "dd090010180200001c0000"
    )

    # Common N capabilities
    n_capabilities = [
        capabilities.N_CAPABILITY_LDPC,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_RX_STBC1,
        capabilities.N_CAPABILITY_MAX_AMSDU_7935,
        capabilities.N_CAPABILITY_DSSS_CCK_40,
        capabilities.N_CAPABILITY_SHORT_GI_20,
    ]

    if channel <= 11:
        band = Band.BAND_2G
        phy_mode: HtMode | VhtMode = HtMode(bw=20)
        supported_rates = SupportedRates.CCK_AND_OFDM
        basic_rates = BasicRate.CCK_AND_OFDM
        ac_capabilities = None
    else:
        band = Band.BAND_5G
        phy_mode = VhtMode(bw=20)
        supported_rates = SupportedRates.OFDM
        basic_rates = BasicRate.OFDM_ONLY
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
                channel=BssChannel(band, channel, phy_mode),
                bss_settings=[
                    BssSettings(
                        ssid=ssid,
                        security=security,
                        password=password,
                        custom_uci_options={
                            "dtim_period": 3,
                            "vendor_elements": [vendor_elements],
                            "preamble": False,
                            "uapsd_advertisement_enabled": True,
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": supported_rates,
                    "basic_rates": basic_rates,
                },
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
                ac_capabilities=CapabilitySelection.CUSTOM(ac_capabilities)
                if ac_capabilities
                else CapabilitySelection.DEFAULT(),
            )
        ]
    )


def asus_rtac86u(
    channel: int,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Asus RTAC86U.

    Supported: 2.4GHz and 5GHz, Open or WPA2.
    """

    if channel <= 11:
        band = Band.BAND_2G
        vendor_elements = "42020000"
        supported_rates = SupportedRates.CCK_AND_OFDM
        basic_rates = BasicRate.CCK_AND_OFDM
        ieee80211h = False
    else:
        band = Band.BAND_5G
        vendor_elements = (
            "074255532024011e28011e2c011e30011e34011e38011e3c011e40011e64011e"
            "68011e6c011e70011e74011e84011e88011e8c011e95011e99011e9d011ea1011e"
            "a5011e"
            "23021300"
            "42020000"
        )
        supported_rates = SupportedRates.OFDM
        basic_rates = BasicRate.OFDM_ONLY
        ieee80211h = True

    custom_uci_options: UciRadioOptions = {
        "beacon_int": 100,
        "supported_rates": supported_rates,
        "basic_rates": basic_rates,
        "ieee80211h": ieee80211h,
    }
    if ieee80211h:
        custom_uci_options["ieee80211d"] = True
        custom_uci_options["spectrum_mgmt_required"] = True
        custom_uci_options["local_pwr_constraint"] = 0

    custom_hostapd_options: HostapdOptions = {
        "bss_load_update_period": 50,
        "chan_util_avg_period": 600,
    }

    return AccessPointConfig(
        radios=[
            RadioConfig(
                channel=BssChannel(band, channel, LegacyMode()),
                bss_settings=[
                    BssSettings(
                        ssid=ssid,
                        security=security,
                        password=password,
                        custom_uci_options={
                            "dtim_period": 3,
                            "vendor_elements": [vendor_elements],
                            "preamble": False,
                        },
                    )
                ],
                custom_uci_options=custom_uci_options,
                custom_hostapd_options=custom_hostapd_options,
            )
        ]
    )


def asus_rtac5300(
    channel: int,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Asus RTAC5300.

    Supported: 2.4GHz and 5GHz, Open or WPA2.
    """
    if channel <= 11:
        band = Band.BAND_2G
        phy_mode: HtMode | VhtMode = HtMode(bw=20)
        supported_rates = SupportedRates.CCK_AND_OFDM
        basic_rates = BasicRate.CCK_AND_OFDM
        vendor_elements = (
            "dd090110180200009c0000"
            "dd25f832e4010101020100031411b5"
            "2fd437509c30b3d7f5cf5754fb125aed3b8507045aed3b85"
            "dd1e00904c0418bf0cb2798b0faaff0000aaff0000c0050001000000c3020002"
        )
        ac_capabilities = None
    else:
        band = Band.BAND_5G
        phy_mode = VhtMode(bw=20)
        supported_rates = SupportedRates.OFDM
        basic_rates = BasicRate.OFDM_ONLY
        vendor_elements = "dd090110180200009c0000" + "dd0500904c0410"
        ac_capabilities = [
            capabilities.AC_CAPABILITY_RXLDPC,
            capabilities.AC_CAPABILITY_SHORT_GI_80,
            capabilities.AC_CAPABILITY_TX_STBC_2BY1,
            capabilities.AC_CAPABILITY_RX_STBC_1,
            capabilities.AC_CAPABILITY_MAX_MPDU_11454,
            capabilities.AC_CAPABILITY_MAX_A_MPDU_LEN_EXP7,
        ]

    n_capabilities = [
        capabilities.N_CAPABILITY_LDPC,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_RX_STBC1,
        capabilities.N_CAPABILITY_SHORT_GI_20,
    ]

    custom_hostapd_options: HostapdOptions = {
        "bss_load_update_period": 50,
        "chan_util_avg_period": 600,
    }

    return AccessPointConfig(
        radios=[
            RadioConfig(
                channel=BssChannel(band, channel, phy_mode),
                bss_settings=[
                    BssSettings(
                        ssid=ssid,
                        security=security,
                        password=password,
                        custom_uci_options={
                            "dtim_period": 3,
                            "vendor_elements": [vendor_elements],
                            "preamble": False,
                            "uapsd_advertisement_enabled": True,
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": supported_rates,
                    "basic_rates": basic_rates,
                },
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
                ac_capabilities=CapabilitySelection.CUSTOM(ac_capabilities)
                if ac_capabilities
                else CapabilitySelection.DEFAULT(),
                custom_hostapd_options=custom_hostapd_options,
            )
        ]
    )


def asus_rtn56u(
    channel: int,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Asus RTN56U.

    Supported: 2.4GHz and 5GHz, Open or WPA2.
    """
    if channel <= 11:
        band = Band.BAND_2G
        supported_rates = SupportedRates.CCK_AND_OFDM
        basic_rate = BasicRate.CCK_AND_OFDM
        vendor_elements = (
            "dd07000c4307000000"
            "0706555320010b14"
            "33082001020304050607"
            "33082105060708090a0b"
            "dd270050f204104a000110104400010210470010bc329e001dd811b286011c872cd33448103c000101"
        )
    else:
        band = Band.BAND_5G
        supported_rates = SupportedRates.OFDM
        basic_rate = BasicRate.OFDM_ONLY
        vendor_elements = "dd07000c4307000000" + "0706555320010b14"

    n_capabilities = [
        capabilities.N_CAPABILITY_SHORT_GI_20,
        capabilities.N_CAPABILITY_SHORT_GI_40,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_RX_STBC1,
    ]

    custom_hostapd_options: HostapdOptions = {
        "bss_load_update_period": 50,
        "chan_util_avg_period": 600,
    }

    return AccessPointConfig(
        radios=[
            RadioConfig(
                channel=BssChannel(band, channel, HtMode(bw=20)),  # Always 11n
                bss_settings=[
                    BssSettings(
                        ssid=ssid,
                        security=security,
                        password=password,
                        custom_uci_options={
                            "dtim_period": 1,
                            "vendor_elements": [vendor_elements],
                            "preamble": False,
                            "uapsd_advertisement_enabled": True,
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": supported_rates,
                    "basic_rates": basic_rate,
                },
                n_capabilities=CapabilitySelection.CUSTOM(n_capabilities),
                custom_hostapd_options=custom_hostapd_options,
            )
        ]
    )


def asus_rtn66u(
    channel: int,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Asus RTN66U.

    Supported: 2.4GHz and 5GHz, Open or WPA2.
    """
    if channel <= 11:
        band = Band.BAND_2G
        supported_rates = SupportedRates.CCK_AND_OFDM
        basic_rate = BasicRate.CCK_AND_OFDM
        n_caps = [
            capabilities.N_CAPABILITY_LDPC,
            capabilities.N_CAPABILITY_SHORT_GI_20,
            capabilities.N_CAPABILITY_TX_STBC,
            capabilities.N_CAPABILITY_RX_STBC1,
            capabilities.N_CAPABILITY_MAX_AMSDU_7935,
            capabilities.N_CAPABILITY_DSSS_CCK_40,
        ]
    else:
        band = Band.BAND_5G
        supported_rates = SupportedRates.OFDM
        basic_rate = BasicRate.OFDM_ONLY
        n_caps = [
            capabilities.N_CAPABILITY_LDPC,
            capabilities.N_CAPABILITY_SHORT_GI_20,
            capabilities.N_CAPABILITY_TX_STBC,
            capabilities.N_CAPABILITY_RX_STBC1,
            capabilities.N_CAPABILITY_MAX_AMSDU_7935,
        ]

    vendor_elements = "dd090010180200001c0000"

    return AccessPointConfig(
        radios=[
            RadioConfig(
                channel=BssChannel(band, channel, HtMode(bw=20)),  # Always 11n
                bss_settings=[
                    BssSettings(
                        ssid=ssid,
                        security=security,
                        password=password,
                        custom_uci_options={
                            "dtim_period": 3,
                            "vendor_elements": [vendor_elements],
                            "preamble": False,
                            "uapsd_advertisement_enabled": True,
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": supported_rates,
                    "basic_rates": basic_rate,
                },
                n_capabilities=CapabilitySelection.CUSTOM(n_caps),
            )
        ]
    )


# LINT.ThenChange(//src/testing/end_to_end/antlion/packages/antlion/controllers/ap_lib/third_party_ap_profiles/asus.py)
