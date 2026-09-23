#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Controller for Open WRT access point."""

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from __future__ import annotations

import logging
import random
import re
import time
from typing import Literal

import yaml
from antlion.controllers.openwrt_lib import (
    network_settings,
    wireless_config,
    wireless_settings_applier,
)
from antlion.controllers.openwrt_lib.openwrt_constants import (
    SYSTEM_INFO_CMD,
)
from antlion.controllers.openwrt_lib.openwrt_constants import (
    OpenWrtModelMap as modelmap,
)
from antlion.controllers.openwrt_lib.openwrt_constants import (
    OpenWrtWifiSetting,
)
from antlion.types import ControllerConfig, Json
from libs.ssh import connection, settings
from mobly import logger, signals

MOBLY_CONTROLLER_CONFIG_NAME: str = "OpenWrtAP"
ACTS_CONTROLLER_REFERENCE_NAME = "access_points"
OWE_SECURITY = "owe"
SAE_SECURITY = "sae"
SAEMIXED_SECURITY = "sae-mixed"
ENABLE_RADIO = "0"
PMF_ENABLED = 2
WAIT_TIME = 20
DEFAULT_RADIOS = ("radio0", "radio1")


def create(configs: list[ControllerConfig]) -> list[OpenWrtAP]:
    """Creates ap controllers from a json config.

    Creates an ap controller from either a list, or a single element. The element
    can either be just the hostname or a dictionary containing the hostname and
    username of the AP to connect to over SSH.

    Args:
      configs: The json configs that represent this controller.

    Returns:
      OpenWrtAP objects

    Example:
      Below is the config file entry for OpenWrtAP as a list. A testbed can have
      1 or more APs to configure. Each AP has a "ssh_config" key to provide SSH
      login information. OpenWrtAP#__init__() uses this to create SSH object.

        "OpenWrtAP": [
          {
            "ssh_config": {
              "user" : "root",
              "host" : "192.168.1.1"
            }
          },
          {
            "ssh_config": {
              "user" : "root",
              "host" : "192.168.1.2"
            }
          }
        ]
    """
    return [OpenWrtAP(c) for c in configs]


def destroy(objects: list[OpenWrtAP]) -> None:
    """Destroys a list of OpenWrtAP.

    Args:
      aps: The list of OpenWrtAP to destroy.
    """
    for ap in objects:
        ap.close()
        ap.close_ssh()


def get_info(objects: list[OpenWrtAP]) -> list[Json]:
    """Get information on a list of access points.

    Args:
      aps: A list of OpenWrtAP.

    Returns:
      A list of all aps hostname.
    """
    return [ap.ssh_settings.hostname for ap in objects]


BSSIDMap = dict[Literal["2g", "5g"], dict[str, str]]


class OpenWrtAP(object):
    """An OpenWrtAP controller.

    Attributes:
      ssh: The ssh connection to the AP.
      ssh_settings: The ssh settings being used by the ssh connection.
      log: Logging object for OpenWrtAP.
      wireless_setting: object holding wireless configuration.
      network_setting: Object for network configuration.
      model: OpenWrt HW model.
      radios: Fit interface for test.
    """

    def __init__(self, config):
        """Initialize AP."""
        self.ssh_settings = settings.from_config(config["ssh_config"])
        self.ssh = connection.SshConnection(self.ssh_settings)
        self.log = logger.PrefixLoggerAdapter(
            logging.getLogger(),
            {
                logger.PrefixLoggerAdapter.EXTRA_KEY_LOG_PREFIX: f"[OpenWrtAP|{self.ssh_settings.hostname}]",
            },
        )
        self.wireless_setting: (
            wireless_settings_applier.WirelessSettingsApplier | None
        ) = None
        self.network_setting = network_settings.NetworkSettings(
            self.ssh, self.ssh_settings, self.log
        )
        self.model = self.get_model_name()
        if self.model in modelmap.__dict__:
            self.radios = modelmap.__dict__[self.model]
        else:
            self.radios = DEFAULT_RADIOS

    def configure_ap(
        self,
        wireless_configs: list[wireless_config.WirelessConfig],
        channel_2g: int,
        channel_5g: int,
    ):
        """Configure AP with the required settings.

        Each test class inherits WifiBaseTest. Based on the test, we may need to
        configure PSK, WEP, OPEN, ENT networks on 2G and 5G bands in any
        combination. We call WifiBaseTest methods get_psk_network(),
        get_open_network(), get_wep_network() and get_ent_network() to create
        dictionaries which contains this information. 'wifi_configs' is a list of
        such dictionaries. Example below configures 2 WiFi networks - 1 PSK 2G and
        1 Open 5G on one AP. configure_ap() is called from WifiBaseTest to
        configure the APs.

        wifi_configs = [
          {
            '2g': {
              'SSID': '2g_AkqXWPK4',
              'security': 'psk2',
              'password': 'YgYuXqDO9H',
              'hiddenSSID': False
            },
          },
          {
            '5g': {
              'SSID': '5g_8IcMR1Sg',
              'security': 'none',
              'hiddenSSID': False
            },
          }
        ]

        Args:
          wifi_configs: list of network settings for 2G and 5G bands.
          channel_2g: channel for 2G band.
          channel_5g: channel for 5G band.
        """
        self.wireless_setting = (
            wireless_settings_applier.WirelessSettingsApplier(
                self.ssh,
                wireless_configs,
                channel_2g,
                channel_5g,
                self.radios[1],
                self.radios[0],
            )
        )
        self.wireless_setting.apply_wireless_settings()

    def start_ap(self):
        """Starts the AP with the settings in /etc/config/wireless."""
        self.ssh.run("wifi up")
        curr_time = time.time()
        while time.time() < curr_time + WAIT_TIME:
            if self.get_wifi_status():
                return
            time.sleep(3)
        if not self.get_wifi_status():
            raise ValueError("Failed to turn on WiFi on the AP.")

    def stop_ap(self):
        """Stops the AP."""
        self.ssh.run("wifi down")
        curr_time = time.time()
        while time.time() < curr_time + WAIT_TIME:
            if not self.get_wifi_status():
                return
            time.sleep(3)
        if self.get_wifi_status():
            raise ValueError("Failed to turn off WiFi on the AP.")

    def get_bssids_for_wifi_networks(self) -> BSSIDMap:
        """Get BSSIDs for wifi networks configured.

        Returns:
          Dictionary of SSID - BSSID map for both bands.
        """
        bssid_map: BSSIDMap = {"2g": {}, "5g": {}}
        for radio in self.radios:
            ssid_ifname_map = self.get_ifnames_for_ssids(radio)
            if radio == self.radios[0]:
                for ssid, ifname in ssid_ifname_map.items():
                    bssid_map["5g"][ssid] = self.get_bssid(ifname)
            elif radio == self.radios[1]:
                for ssid, ifname in ssid_ifname_map.items():
                    bssid_map["2g"][ssid] = self.get_bssid(ifname)
        return bssid_map

    def get_ifnames_for_ssids(self, radio) -> dict[str, str]:
        """Get interfaces for wifi networks.

        Args:
          radio: 2g or 5g radio get the bssids from.

        Returns:
          dictionary of ssid - ifname mappings.
        """
        ssid_ifname_map: dict[str, str] = {}
        str_output = self.ssh.run(f"wifi status {radio}").stdout.decode("utf-8")
        wifi_status = yaml.load(
            str_output.replace("\t", "").replace("\n", ""),
            Loader=yaml.SafeLoader,
        )
        wifi_status = wifi_status[radio]
        if wifi_status["up"]:
            interfaces = wifi_status["interfaces"]
            for config in interfaces:
                ssid = config["config"]["ssid"]
                ifname = config["ifname"]
                ssid_ifname_map[ssid] = ifname
        return ssid_ifname_map

    def get_bssid(self, ifname):
        """Get MAC address from an interface.

        Args:
          ifname: interface name of the corresponding MAC.

        Returns:
          BSSID of the interface.
        """
        ifconfig = self.ssh.run(f"ifconfig {ifname}").stdout.decode("utf-8")
        mac_addr = ifconfig.split("\n")[0].split()[-1]
        return mac_addr

    def set_wpa_encryption(self, encryption):
        """Set different encryptions to wpa or wpa2.

        Args:
          encryption: ccmp, tkip, or ccmp+tkip.
        """
        str_output = self.ssh.run("wifi status").stdout.decode("utf-8")
        wifi_status = yaml.load(
            str_output.replace("\t", "").replace("\n", ""),
            Loader=yaml.SafeLoader,
        )

        # Counting how many interface are enabled.
        total_interface = 0
        for radio in self.radios:
            num_interface = len(wifi_status[radio]["interfaces"])
            total_interface += num_interface

        # Iterates every interface to get and set wpa encryption.
        default_extra_interface = 2
        for i in range(total_interface + default_extra_interface):
            origin_encryption = self.ssh.run(
                f"uci get wireless.@wifi-iface[{i}].encryption"
            ).stdout.decode("utf-8")
            origin_psk_pattern = re.match(r"psk\b", origin_encryption)
            target_psk_pattern = re.match(r"psk\b", encryption)
            origin_psk2_pattern = re.match(r"psk2\b", origin_encryption)
            target_psk2_pattern = re.match(r"psk2\b", encryption)

            if origin_psk_pattern == target_psk_pattern:
                self.ssh.run(
                    f"uci set wireless.@wifi-iface[{i}].encryption={encryption}"
                )

            if origin_psk2_pattern == target_psk2_pattern:
                self.ssh.run(
                    f"uci set wireless.@wifi-iface[{i}].encryption={encryption}"
                )

        self.ssh.run("uci commit wireless")
        self.ssh.run("wifi")

    def set_password(self, pwd_5g=None, pwd_2g=None):
        """Set password for individual interface.

        Args:
            pwd_5g: 8 ~ 63 chars, ascii letters and digits password for 5g network.
            pwd_2g: 8 ~ 63 chars, ascii letters and digits password for 2g network.
        """
        if pwd_5g:
            if len(pwd_5g) < 8 or len(pwd_5g) > 63:
                self.log.error("Password must be 8~63 characters long")
            # Only accept ascii letters and digits
            elif not re.match("^[A-Za-z0-9]*$", pwd_5g):
                self.log.error(
                    "Password must only contains ascii letters and digits"
                )
            else:
                self.ssh.run(f"uci set wireless.@wifi-iface[{3}].key={pwd_5g}")
                self.log.info(f"Set 5G password to :{pwd_5g}")

        if pwd_2g:
            if len(pwd_2g) < 8 or len(pwd_2g) > 63:
                self.log.error("Password must be 8~63 characters long")
            # Only accept ascii letters and digits
            elif not re.match("^[A-Za-z0-9]*$", pwd_2g):
                self.log.error(
                    "Password must only contains ascii letters and digits"
                )
            else:
                self.ssh.run(f"uci set wireless.@wifi-iface[{2}].key={pwd_2g}")
                self.log.info(f"Set 2G password to :{pwd_2g}")

        self.ssh.run("uci commit wireless")
        self.ssh.run("wifi")

    def set_ssid(self, ssid_5g=None, ssid_2g=None):
        """Set SSID for individual interface.

        Args:
            ssid_5g: 8 ~ 63 chars for 5g network.
            ssid_2g: 8 ~ 63 chars for 2g network.
        """
        if ssid_5g:
            if len(ssid_5g) < 8 or len(ssid_5g) > 63:
                self.log.error("SSID must be 8~63 characters long")
            # Only accept ascii letters and digits
            else:
                self.ssh.run(
                    f"uci set wireless.@wifi-iface[{3}].ssid={ssid_5g}"
                )
                self.log.info(f"Set 5G SSID to :{ssid_5g}")

        if ssid_2g:
            if len(ssid_2g) < 8 or len(ssid_2g) > 63:
                self.log.error("SSID must be 8~63 characters long")
            # Only accept ascii letters and digits
            else:
                self.ssh.run(
                    f"uci set wireless.@wifi-iface[{2}].ssid={ssid_2g}"
                )
                self.log.info(f"Set 2G SSID to :{ssid_2g}")

        self.ssh.run("uci commit wireless")
        self.ssh.run("wifi")

    def generate_mobility_domain(self):
        """Generate 4-character hexadecimal ID.

        Returns:
          String; a 4-character hexadecimal ID.
        """
        md = f"{random.getrandbits(16):04x}"
        self.log.info(f"Mobility Domain ID: {md}")
        return md

    def enable_80211r(self, iface, md):
        """Enable 802.11r for one single radio.

        Args:
          iface: index number of wifi-iface.
                  2: radio1
                  3: radio0
          md: mobility domain. a 4-character hexadecimal ID.
        Raises:
          TestSkip if 2g or 5g radio is not up or 802.11r is not enabled.
        """
        str_output = self.ssh.run("wifi status").stdout.decode("utf-8")
        wifi_status = yaml.load(
            str_output.replace("\t", "").replace("\n", ""),
            Loader=yaml.SafeLoader,
        )
        # Check if the radio is up.
        if iface == OpenWrtWifiSetting.IFACE_2G:
            if wifi_status[self.radios[1]]["up"]:
                self.log.info("2g network is ENABLED")
            else:
                raise signals.TestSkip("2g network is NOT ENABLED")
        elif iface == OpenWrtWifiSetting.IFACE_5G:
            if wifi_status[self.radios[0]]["up"]:
                self.log.info("5g network is ENABLED")
            else:
                raise signals.TestSkip("5g network is NOT ENABLED")

        # Setup 802.11r.
        self.ssh.run(f"uci set wireless.@wifi-iface[{iface}].ieee80211r='1'")
        self.ssh.run(
            f"uci set wireless.@wifi-iface[{iface}].ft_psk_generate_local='1'"
        )
        self.ssh.run(
            f"uci set wireless.@wifi-iface[{iface}].mobility_domain='{md}'"
        )
        self.ssh.run("uci commit wireless")
        self.ssh.run("wifi")

        # Check if 802.11r is enabled.
        result = self.ssh.run(
            f"uci get wireless.@wifi-iface[{iface}].ieee80211r"
        ).stdout.decode("utf-8")
        if result == "1":
            self.log.info("802.11r is ENABLED")
        else:
            raise signals.TestSkip("802.11r is NOT ENABLED")

    def get_wifi_network(self, security=None, band=None):
        """Return first match wifi interface's config.

        Args:
          security: psk2 or none
          band: '2g' or '5g'

        Returns:
          A dict contains match wifi interface's config.
        """
        if not self.wireless_setting:
            raise RuntimeError(
                "The AP has not been configured yet; run configure_ap()"
            )

        for wifi_iface in self.wireless_setting.wireless_configs:
            match_list = []
            wifi_network = wifi_iface.__dict__
            if security:
                match_list.append(security == wifi_network["security"])
            if band:
                match_list.append(band == wifi_network["band"])

            if all(match_list):
                wifi_network["SSID"] = wifi_network["ssid"]
                if not wifi_network["password"]:
                    del wifi_network["password"]
                return wifi_network
        return None

    def get_wifi_status(self):
        """Check if radios are up. Default are 2G and 5G bands.

        Returns:
          True if both radios are up. False if not.
        """
        status = True
        for radio in self.radios:
            try:
                str_output = self.ssh.run(f"wifi status {radio}").stdout.decode(
                    "utf-8"
                )
                wifi_status = yaml.load(
                    str_output.replace("\t", "").replace("\n", ""),
                    Loader=yaml.SafeLoader,
                )
                status = wifi_status[radio]["up"] and status
            except:
                self.log.info("Failed to make ssh connection to the OpenWrt")
                return False
        return status

    def verify_wifi_status(self, timeout=20):
        """Ensure wifi interfaces are ready.

        Args:
          timeout: An integer that is the number of times to try
                   wait for interface ready.
        Returns:
          True if both radios are up. False if not.
        """
        start_time = time.time()
        end_time = start_time + timeout
        while time.time() < end_time:
            if self.get_wifi_status():
                return True
            time.sleep(1)
        return False

    def get_model_name(self):
        """Get Openwrt model name.

        Returns:
          A string include device brand and model. e.g. NETGEAR_R8000
        """
        out = self.ssh.run(SYSTEM_INFO_CMD).stdout.decode("utf-8").split("\n")
        for line in out:
            if "board_name" in line:
                model = line.split()[1].strip('",').split(",")
                return "_".join(map(lambda i: i.upper(), model))
        self.log.info("Failed to retrieve OpenWrt model information.")
        return None

    def close(self):
        """Reset wireless and network settings to default and stop AP."""
        if self.network_setting.config:
            self.network_setting.cleanup_network_settings()
        if self.wireless_setting:
            self.wireless_setting.cleanup_wireless_settings()

    def close_ssh(self):
        """Close SSH connection to AP."""
        self.ssh.close()

    def reboot(self):
        """Reboot Openwrt."""
        self.ssh.run("reboot")
