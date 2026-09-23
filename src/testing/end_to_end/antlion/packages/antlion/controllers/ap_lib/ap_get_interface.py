#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
from typing import TYPE_CHECKING

from libs.proc.runner import CalledProcessError

if TYPE_CHECKING:
    from antlion.controllers.access_point import AccessPoint

GET_ALL_INTERFACE = "ls /sys/class/net"
GET_VIRTUAL_INTERFACE = "ls /sys/devices/virtual/net"
BRCTL_SHOW = "brctl show"


class ApInterfacesError(Exception):
    """Error related to AP interfaces."""


class ApInterfaces(object):
    """Class to get network interface information for the device."""

    def __init__(
        self, ap: "AccessPoint", wan_interface_override: str | None = None
    ) -> None:
        """Initialize the ApInterface class.

        Args:
            ap: the ap object within ACTS
            wan_interface_override: wan interface to use if specified by config
        """
        self.ssh = ap.ssh
        self.wan_interface_override = wan_interface_override

    def get_all_interface(self) -> list[str]:
        """Get all network interfaces on the device.

        Returns:
            interfaces_all: list of all the network interfaces on device
        """
        output = self.ssh.run(GET_ALL_INTERFACE)
        interfaces_all = output.stdout.decode("utf-8").split("\n")

        return interfaces_all

    def get_virtual_interface(self) -> list[str]:
        """Get all virtual interfaces on the device.

        Returns:
            interfaces_virtual: list of all the virtual interfaces on device
        """
        output = self.ssh.run(GET_VIRTUAL_INTERFACE)
        interfaces_virtual = output.stdout.decode("utf-8").split("\n")

        return interfaces_virtual

    def get_physical_interface(self) -> list[str]:
        """Get all the physical interfaces of the device.

        Get all physical interfaces such as eth ports and wlan ports

        Returns:
            interfaces_phy: list of all the physical interfaces
        """
        interfaces_all = self.get_all_interface()
        interfaces_virtual = self.get_virtual_interface()
        interfaces_phy = list(set(interfaces_all) - set(interfaces_virtual))

        return interfaces_phy

    def get_bridge_interface(self) -> list[str]:
        """Get all the bridge interfaces of the device.

        Returns:
            interfaces_bridge: the list of bridge interfaces, return None if
                bridge utility is not available on the device

        Raises:
            ApInterfaceError: Failing to run brctl
        """
        try:
            output = self.ssh.run(BRCTL_SHOW)
        except CalledProcessError as e:
            raise ApInterfacesError(f'failed to execute "{BRCTL_SHOW}"') from e

        lines = output.stdout.decode("utf-8").split("\n")
        interfaces_bridge = []
        for line in lines:
            interfaces_bridge.append(line.split("\t")[0])
        interfaces_bridge.pop(0)
        return [x for x in interfaces_bridge if x != ""]

    def get_wlan_interface(self) -> tuple[str, str]:
        """Get all WLAN interfaces and specify 2.4 GHz and 5 GHz interfaces.

        Returns:
            interfaces_wlan: all wlan interfaces
        Raises:
            ApInterfacesError: Missing at least one WLAN interface
        """
        wlan_2g = None
        wlan_5g = None
        interfaces_phy = self.get_physical_interface()
        for iface in interfaces_phy:
            output = self.ssh.run(f"iwlist {iface} freq")
            if (
                b"Channel 06" in output.stdout
                and b"Channel 36" not in output.stdout
            ):
                wlan_2g = iface
            elif (
                b"Channel 36" in output.stdout
                and b"Channel 06" not in output.stdout
            ):
                wlan_5g = iface

        if wlan_2g is None or wlan_5g is None:
            raise ApInterfacesError("Missing at least one WLAN interface")

        return (wlan_2g, wlan_5g)

    def get_wan_interface(self) -> str:
        """Get the WAN interface which has internet connectivity. If a wan
        interface is already specified return that instead.

        Returns:
            wan: the only one WAN interface
        Raises:
            ApInterfacesError: no running WAN can be found
        """
        if self.wan_interface_override:
            return self.wan_interface_override

        wan = None
        interfaces_phy = self.get_physical_interface()
        interfaces_wlan = self.get_wlan_interface()
        interfaces_eth = list(set(interfaces_phy) - set(interfaces_wlan))
        for iface in interfaces_eth:
            network_status = self.check_ping(iface)
            if network_status == 1:
                wan = iface
                break
        if wan:
            return wan

        output = self.ssh.run("ifconfig")
        interfaces_all = output.stdout.decode("utf-8").split("\n")
        logging.info(f"IFCONFIG output = {interfaces_all}")

        raise ApInterfacesError("No WAN interface available")

    def get_lan_interface(self) -> str | None:
        """Get the LAN interface connecting to local devices.

        Returns:
            lan: the only one running LAN interface of the devices
            None, if nothing was found.
        """
        lan = None
        interfaces_phy = self.get_physical_interface()
        interfaces_wlan = self.get_wlan_interface()
        interfaces_eth = list(set(interfaces_phy) - set(interfaces_wlan))
        interface_wan = self.get_wan_interface()
        interfaces_eth.remove(interface_wan)
        for iface in interfaces_eth:
            output = self.ssh.run(f"ifconfig {iface}")
            if b"RUNNING" in output.stdout:
                lan = iface
                break
        return lan

    def check_ping(self, iface: str) -> int:
        """Check the ping status on specific interface to determine the WAN.

        Args:
            iface: the specific interface to check
        Returns:
            network_status: the connectivity status of the interface
        """
        try:
            self.ssh.run(f"ping -c 3 -I {iface} 8.8.8.8")
            return 1
        except CalledProcessError:
            return 0
