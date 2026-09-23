# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
# LINT.IfChange


from antlion.controllers.ap_lib import (
    hostapd_config,
    hostapd_constants,
    hostapd_utils,
)
from antlion.controllers.ap_lib.hostapd_security import Security, SecurityMode


def actiontec_pk5000(
    iface_wlan_2g: str,
    channel: int,
    security: Security,
    ssid: str | None = None,
) -> hostapd_config.HostapdConfig:
    """A simulated implementation of what a Actiontec PK5000 AP
    Args:
        iface_wlan_2g: The 2.4 interface of the test AP.
        channel: What channel to use.  Only 2.4Ghz is supported for this profile
        security: A security profile.  Must be open or WPA2 as this is what is
            supported by the PK5000.
        ssid: Network name
    Returns:
        A hostapd config

    Differences from real pk5000:
        Supported Rates IE:
            PK5000: Supported: 1, 2, 5.5, 11
                    Extended: 6, 9, 12, 18, 24, 36, 48, 54
            Simulated: Supported: 1, 2, 5.5, 11, 6, 9, 12, 18
                       Extended: 24, 36, 48, 54
    """
    if channel > 11:
        # Technically this should be 14 but since the PK5000 is a US only AP,
        # 11 is the highest allowable channel.
        raise ValueError(
            f"The Actiontec PK5000 does not support 5Ghz. Invalid channel ({channel})"
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

    interface = iface_wlan_2g
    short_preamble = False
    force_wmm = False
    beacon_interval = 100
    dtim_period = 3
    # Sets the basic rates and supported rates of the PK5000
    additional_params = (
        hostapd_constants.CCK_AND_OFDM_BASIC_RATES
        | hostapd_constants.CCK_AND_OFDM_DATA_RATES
    )

    config = hostapd_config.HostapdConfig(
        ssid=ssid,
        channel=channel,
        hidden=False,
        security=security,
        interface=interface,
        mode=hostapd_constants.Mode.MODE_11G,
        force_wmm=force_wmm,
        beacon_interval=beacon_interval,
        dtim_period=dtim_period,
        short_preamble=short_preamble,
        additional_parameters=additional_params,
    )

    return config


def actiontec_mi424wr(
    iface_wlan_2g: str,
    channel: int,
    security: Security,
    ssid: str | None = None,
) -> hostapd_config.HostapdConfig:
    # TODO(b/143104825): Permit RIFS once it is supported
    """A simulated implementation of an Actiontec MI424WR AP.
    Args:
        iface_wlan_2g: The 2.4Ghz interface of the test AP.
        channel: What channel to use (2.4Ghz or 5Ghz).
        security: A security profile.
        ssid: The network name.
    Returns:
        A hostapd config.

    Differences from real MI424WR:
        HT Capabilities:
            MI424WR:
                HT Rx STBC: Support for 1, 2, and 3
            Simulated:
                HT Rx STBC: Support for 1
        HT Information:
            MI424WR:
                RIFS: Premitted
            Simulated:
                RIFS: Prohibited
    """
    if channel > 11:
        raise ValueError(
            f"The Actiontec MI424WR does not support 5Ghz. Invalid channel ({channel})"
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
        hostapd_constants.N_CAPABILITY_TX_STBC,
        hostapd_constants.N_CAPABILITY_DSSS_CCK_40,
        hostapd_constants.N_CAPABILITY_RX_STBC1,
    ]
    rates = (
        hostapd_constants.CCK_AND_OFDM_DATA_RATES
        | hostapd_constants.CCK_AND_OFDM_BASIC_RATES
    )
    # Proprietary Atheros Communication: Adv Capability IE
    # Proprietary Atheros Communication: Unknown IE
    # Country Info: US Only IE
    vendor_elements = {
        "vendor_elements": "dd0900037f01010000ff7f"
        "dd0a00037f04010000000000"
        "0706555320010b1b"
    }

    additional_params = rates | vendor_elements

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
        n_capabilities=n_capabilities,
        additional_parameters=additional_params,
    )

    return config


# LINT.ThenChange(//src/testing/end_to_end/mobly_controller/openwrt_access_point/lib/profiles/actiontec.py)
