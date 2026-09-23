# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Class for Wireless config."""

from antlion.controllers.ap_lib.hostapd_security import OpenWRTEncryptionMode


class WirelessConfig(object):
    """Creates an object to hold wireless config.

    Attributes:
      name: name of the wireless config
      ssid: SSID of the network.
      security: security of the wifi network.
      band: band of the wifi network.
      iface: network interface of the wifi network.
      password: password for psk network.
      wep_key: wep keys for wep network.
      wep_key_num: key number for wep network.
      radius_server_ip: IP address of radius server.
      radius_server_port: Port number of radius server.
      radius_server_secret: Secret key of radius server.
      hidden: Boolean, if the wifi network is hidden.
      ieee80211w: PMF bit of the wifi network.
    """

    def __init__(
        self,
        name: str,
        ssid: str,
        security: OpenWRTEncryptionMode,
        band: str,
        iface: str = "lan",
        password: str | None = None,
        wep_key: list[str] | None = None,
        wep_key_num: int = 1,
        radius_server_ip: str | None = None,
        radius_server_port: int | None = None,
        radius_server_secret: str | None = None,
        hidden: bool = False,
        ieee80211w: int | None = None,
    ):
        self.name = name
        self.ssid = ssid
        self.security = security
        self.band = band
        self.iface = iface
        self.password = password
        self.wep_key = wep_key
        self.wep_key_num = wep_key_num
        self.radius_server_ip = radius_server_ip
        self.radius_server_port = radius_server_port
        self.radius_server_secret = radius_server_secret
        self.hidden = hidden
        self.ieee80211w = ieee80211w
