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


def belkin_f9k1001v5(
    channel: BssChannel,
    ssid: str,
    security: Security,
    password: str | None = None,
) -> AccessPointConfig:
    """Simulated profile for Belkin F9K1001v5.

    Supported: 2.4GHz, Open or WPA2.
    """
    if channel.band != Band.BAND_2G:
        raise ValueError("The Belkin F9k1001v5 does not support 5GHz.")

    vendor_elements = (
        "dd090010180200100c0000"
        "dd180050f204104a00011010440001021049000600372a000120"
    )

    n_capabilities = [
        capabilities.N_CAPABILITY_SHORT_GI_20,
        capabilities.N_CAPABILITY_SHORT_GI_40,
        capabilities.N_CAPABILITY_TX_STBC,
        capabilities.N_CAPABILITY_MAX_AMSDU_7935,
        capabilities.N_CAPABILITY_DSSS_CCK_40,
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
                            "dtim_period": 3,
                            "vendor_elements": [vendor_elements],
                            "preamble": False,
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


# LINT.ThenChange(//src/testing/end_to_end/antlion/packages/antlion/controllers/ap_lib/third_party_ap_profiles/belkin.py)
