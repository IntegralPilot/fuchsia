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


def securifi_almond(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Securifi Almond.

    Supported: 2.4GHz, Open or WPA2.
    """
    if channel.band != Band.BAND_2G:
        raise ValueError("Securifi Almond only supports 2.4GHz")

    # Ralink Technology IE, Country Information IE, AP Channel Report IEs
    vendor_elements = (
        "dd07000c4307000000"
        "0706555320010b14"
        "33082001020304050607"
        "33082105060708090a0b"
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
                custom_uci_options={
                    "beacon_int": 100,
                },
                n_capabilities=CapabilitySelection.CUSTOM(
                    [
                        capabilities.N_CAPABILITY_HT40_PLUS,
                        capabilities.N_CAPABILITY_SHORT_GI_20,
                        capabilities.N_CAPABILITY_SHORT_GI_40,
                        capabilities.N_CAPABILITY_TX_STBC,
                        capabilities.N_CAPABILITY_RX_STBC1,
                        capabilities.N_CAPABILITY_DSSS_CCK_40,
                    ]
                ),
                custom_hostapd_options={
                    "bss_load_update_period": 50,
                    "chan_util_avg_period": 600,
                    "obss_interval": 300,
                },
            )
        ]
    )


# LINT.ThenChange(//src/testing/end_to_end/antlion/packages/antlion/controllers/ap_lib/third_party_ap_profiles/securifi.py)
