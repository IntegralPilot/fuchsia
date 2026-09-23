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


def actiontec_pk5000(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Actiontec PK5000.

    Supported: 2.4GHz, Open or WPA2.
    """
    if channel.band != Band.BAND_2G:
        raise ValueError(
            "Actiontec PK5000 only supports 2.4GHz (channels 1-11)"
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
                            "dtim_period": 3,
                            "preamble": True,
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": SupportedRates.CCK_AND_OFDM,
                    "basic_rates": BasicRate.CCK_AND_OFDM,
                },
            )
        ]
    )


def actiontec_mi424wr(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Actiontec MI424WR.

    Supported: 2.4GHz, Open or WPA2.
    """
    if channel.band != Band.BAND_2G:
        raise ValueError(
            "Actiontec MI424WR only supports 2.4GHz (channels 1-11)"
        )

    vendor_elements = (
        "dd0900037f01010000ff7f" "dd0a00037f04010000000000" "0706555320010b1b"
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
                            # TODO(b/513469970): Investigate removal of 'wmm' option.
                        },
                    )
                ],
                custom_uci_options={
                    "beacon_int": 100,
                    "supported_rates": SupportedRates.CCK_AND_OFDM,
                    "basic_rates": BasicRate.CCK_AND_OFDM,
                },
                n_capabilities=CapabilitySelection.CUSTOM(
                    [
                        capabilities.N_CAPABILITY_TX_STBC,
                        capabilities.N_CAPABILITY_DSSS_CCK_40,
                        capabilities.N_CAPABILITY_RX_STBC1,
                    ]
                ),
            )
        ]
    )


# LINT.ThenChange(//src/testing/end_to_end/antlion/packages/antlion/controllers/ap_lib/third_party_ap_profiles/actiontec.py)
