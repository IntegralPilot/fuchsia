# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


from antlion.controllers.ap_lib import (
    hostapd_config,
    hostapd_constants,
    hostapd_utils,
)
from antlion.controllers.ap_lib.hostapd_security import Security, SecurityMode


def securifi_almond(
    iface_wlan_2g: str,
    channel: int,
    security: Security,
    ssid: str | None = None,
) -> hostapd_config.HostapdConfig:
    """A simulated implementation of a Securifi Almond AP
    Args:
        iface_wlan_2g: The 2.4Ghz interface of the test AP.
        channel: What channel to use.
        security: A security profile (open or WPA2).
        ssid: The network name.
    Returns:
        A hostapd config.
    Differences from real Almond:
            Rates:
                Almond:
                    Supported: 1, 2, 5.5, 11, 18, 24, 36, 54
                    Extended: 6, 9, 12, 48
                Simulated:
                    Supported: 1, 2, 5.5, 11, 6, 9, 12, 18
                    Extended: 24, 36, 48, 54
            HT Capab:
                A-MPDU
                    Almond: MPDU Density 4
                    Simulated: MPDU Density 8
            RSN Capab (w/ WPA2):
                Almond:
                    RSN PTKSA Replay Counter Capab: 1
                Simulated:
                    RSN PTKSA Replay Counter Capab: 16
    """
    if channel > 11:
        raise ValueError(
            f"The Securifi Almond does not support 5Ghz. Invalid channel ({channel})"
        )
    # Verify interface and security
    hostapd_utils.verify_interface(
        iface_wlan_2g, hostapd_constants.INTERFACE_2G_LIST
    )
    hostapd_utils.verify_security_mode(
        security, [SecurityMode.OPEN, SecurityMode.WPA2]
    )
    if security.security_mode is not SecurityMode.OPEN:
        hostapd_utils.verify_cipher(
            security, [hostapd_constants.WPA2_DEFAULT_CIPER]
        )

    n_capabilities = [
        hostapd_constants.N_CAPABILITY_HT40_PLUS,
        hostapd_constants.N_CAPABILITY_SGI20,
        hostapd_constants.N_CAPABILITY_SGI40,
        hostapd_constants.N_CAPABILITY_TX_STBC,
        hostapd_constants.N_CAPABILITY_RX_STBC1,
        hostapd_constants.N_CAPABILITY_DSSS_CCK_40,
    ]

    rates = (
        hostapd_constants.CCK_AND_OFDM_BASIC_RATES
        | hostapd_constants.CCK_AND_OFDM_DATA_RATES
    )

    # Ralink Technology IE
    # Country Information IE
    # AP Channel Report IEs
    vendor_elements = {
        "vendor_elements": "dd07000c4307000000"
        "0706555320010b14"
        "33082001020304050607"
        "33082105060708090a0b"
    }

    qbss = {"bss_load_update_period": 50, "chan_util_avg_period": 600}

    additional_params = rates | vendor_elements | qbss

    config = hostapd_config.HostapdConfig(
        ssid=ssid,
        channel=channel,
        hidden=False,
        security=security,
        interface=iface_wlan_2g,
        mode=hostapd_constants.Mode.MODE_11N_MIXED,
        force_wmm=True,
        beacon_interval=100,
        dtim_period=1,
        short_preamble=True,
        obss_interval=300,
        n_capabilities=n_capabilities,
        additional_parameters=additional_params,
    )

    return config
