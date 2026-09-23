#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import ipaddress
import logging
import os
import time
from dataclasses import dataclass
from typing import Any, FrozenSet

from antlion import utils
from antlion.capabilities.ssh import SSHConfig, SSHProvider
from antlion.controllers.ap_lib import hostapd_constants
from antlion.controllers.ap_lib.ap_get_interface import ApInterfaces
from antlion.controllers.ap_lib.ap_iwconfig import ApIwconfig
from antlion.controllers.ap_lib.bridge_interface import BridgeInterface
from antlion.controllers.ap_lib.dhcp_config import DhcpConfig, Subnet
from antlion.controllers.ap_lib.dhcp_server import DhcpServer, NoInterfaceError
from antlion.controllers.ap_lib.extended_capabilities import (
    ExtendedCapabilities,
)
from antlion.controllers.ap_lib.hostapd import Error as HostapdError
from antlion.controllers.ap_lib.hostapd import (
    Hostapd,
    StationStatus,
)
from antlion.controllers.ap_lib.hostapd_ap_preset import create_ap_preset
from antlion.controllers.ap_lib.hostapd_config import HostapdConfig
from antlion.controllers.ap_lib.hostapd_security import Security
from antlion.controllers.ap_lib.radvd import Radvd
from antlion.controllers.ap_lib.radvd_config import RadvdConfig
from antlion.controllers.ap_lib.wireless_network_management import (
    BssTransitionManagementRequest,
)
from antlion.controllers.pdu import PduDevice, get_pdu_port_for_device
from antlion.controllers.utils_lib.commands import (
    ip,
    route,
)
from antlion.controllers.utils_lib.commands.tcpdump import LinuxTcpdumpCommand
from antlion.types import ControllerConfig, Json
from antlion.validation import MapValidator
from honeydew.typing.custom_types import MacAddress
from libs.commands import (
    command,
    journalctl,
)
from libs.commands.date import LinuxDateCommand
from libs.proc.runner import CalledProcessError
from libs.ssh import connection, settings
from mobly import logger

MOBLY_CONTROLLER_CONFIG_NAME: str = "AccessPoint"
ACTS_CONTROLLER_REFERENCE_NAME = "access_points"


class Error(Exception):
    """Error raised when there is a problem with the access point."""


@dataclass
class _ApInstance:
    hostapd: Hostapd
    subnet: Subnet


# These ranges were split this way since each physical radio can have up
# to 8 SSIDs so for the 2GHz radio the DHCP range will be
# 192.168.1 - 8 and the 5Ghz radio will be 192.168.9 - 16
_AP_2GHZ_SUBNET_STR_DEFAULT = "192.168.1.0/24"
_AP_5GHZ_SUBNET_STR_DEFAULT = "192.168.9.0/24"

# The last digit of the ip for the bridge interface
BRIDGE_IP_LAST = "100"


def create(configs: list[ControllerConfig]) -> list[AccessPoint]:
    """Creates ap controllers from a json config.

    Creates an ap controller from either a list, or a single
    element. The element can either be just the hostname or a dictionary
    containing the hostname and username of the ap to connect to over ssh.

    Args:
        The json configs that represent this controller.

    Returns:
        A new AccessPoint.
    """
    return [AccessPoint(c) for c in configs]


def destroy(objects: list[AccessPoint]) -> None:
    """Destroys a list of access points.

    Args:
        aps: The list of access points to destroy.
    """
    for ap in objects:
        ap.close()


def get_info(objects: list[AccessPoint]) -> list[Json]:
    """Get information on a list of access points.

    Args:
        aps: A list of AccessPoints.

    Returns:
        A list of all aps hostname.
    """
    return [ap.ssh_settings.hostname for ap in objects]


class AccessPoint:
    """An access point controller.

    Attributes:
        ssh: The ssh connection to this ap.
        ssh_settings: The ssh settings being used by the ssh connection.
        dhcp_settings: The dhcp server settings being used.
    """

    def __init__(self, config: ControllerConfig) -> None:
        """
        Args:
            configs: configs for the access point from config file.
        """
        c = MapValidator(config)
        self.ssh_settings = settings.from_config(c.get(dict, "ssh_config"))
        self.log = logger.PrefixLoggerAdapter(
            logging.getLogger(),
            {
                logger.PrefixLoggerAdapter.EXTRA_KEY_LOG_PREFIX: f"[Access Point|{self.ssh_settings.hostname}]",
            },
        )
        self.device_pdu_config = c.get(dict, "PduDevice", None)
        self.identifier = self.ssh_settings.hostname

        subnet = MapValidator(c.get(dict, "ap_subnet", {}))
        self._AP_2G_SUBNET_STR = subnet.get(
            str, "2g", _AP_2GHZ_SUBNET_STR_DEFAULT
        )
        self._AP_5G_SUBNET_STR = subnet.get(
            str, "5g", _AP_5GHZ_SUBNET_STR_DEFAULT
        )

        self._AP_2G_SUBNET = Subnet(
            ipaddress.IPv4Network(self._AP_2G_SUBNET_STR)
        )
        self._AP_5G_SUBNET = Subnet(
            ipaddress.IPv4Network(self._AP_5G_SUBNET_STR)
        )

        self.ssh = connection.SshConnection(self.ssh_settings)

        # TODO(http://b/278758876): Replace self.ssh with self.ssh_provider
        self.ssh_provider = SSHProvider(
            SSHConfig(
                self.ssh_settings.username,
                self.ssh_settings.hostname,
                self.ssh_settings.identity_file,
                port=self.ssh_settings.port,
                ssh_binary=self.ssh_settings.executable,
                connect_timeout=90,
            )
        )

        # Singleton utilities for running various commands.
        self._ip_cmd = command.require(ip.LinuxIpCommand(self.ssh))
        self._route_cmd = command.require(route.LinuxRouteCommand(self.ssh))
        self._journalctl_cmd = command.require(
            journalctl.LinuxJournalctlCommand(self.ssh)
        )

        # A map from network interface name to _ApInstance objects representing
        # the hostapd instance running against the interface.
        self._aps: dict[str, _ApInstance] = dict()
        self._dhcp: DhcpServer | None = None
        self._dhcp_bss: dict[str, Subnet] = dict()
        self._radvd: Radvd | None = None
        self.bridge = BridgeInterface(self.ssh)
        self.iwconfig = ApIwconfig(self)

        # Check to see if wan_interface is specified in acts_config for tests
        # isolated from the internet and set this override.
        self.interfaces = ApInterfaces(self, c.get(str, "wan_interface", None))

        # Get needed interface names and initialize the unnecessary ones.
        self.wan = self.interfaces.get_wan_interface()
        self.wlan = self.interfaces.get_wlan_interface()
        self.wlan_2g = self.wlan[0]
        self.wlan_5g = self.wlan[1]
        self.lan = self.interfaces.get_lan_interface()
        self._initial_ap()
        self.setup_bridge = False

        # Allow use of tcpdump
        self.tcpdump = LinuxTcpdumpCommand(self.ssh_provider)

        # Access points are not given internet access, so their system time needs to be
        # manually set to be accurate.
        LinuxDateCommand(self.ssh_provider).sync()

    def _initial_ap(self) -> None:
        """Initial AP interfaces.

        Bring down hostapd if instance is running, bring down all bridge
        interfaces.
        """
        # This is necessary for Gale/Whirlwind flashed with dev channel image
        # Unused interfaces such as existing hostapd daemon, guest, mesh
        # interfaces need to be brought down as part of the AP initialization
        # process, otherwise test would fail.
        try:
            self.ssh.run("stop wpasupplicant")
        except CalledProcessError:
            self.log.info("No wpasupplicant running")
        try:
            self.ssh.run("stop hostapd")
        except CalledProcessError:
            self.log.info("No hostapd running")
        try:
            self.ssh.run("killall dhcpd")
        except CalledProcessError:
            self.log.info("No dhcpd running")
        # Bring down all wireless interfaces
        for iface in self.wlan:
            WLAN_DOWN = f"ip link set {iface} down"
            self.ssh.run(WLAN_DOWN)
        # Bring down all bridge interfaces
        bridge_interfaces = self.interfaces.get_bridge_interface()
        for iface in bridge_interfaces:
            BRIDGE_DOWN = f"ip link set {iface} down"
            BRIDGE_DEL = f"brctl delbr {iface}"
            self.ssh.run(BRIDGE_DOWN)
            self.ssh.run(BRIDGE_DEL)

    def start_ap(
        self,
        hostapd_config: HostapdConfig,
        radvd_config: RadvdConfig | None = None,
        setup_bridge: bool = False,
        is_nat_enabled: bool = True,
        additional_parameters: dict[str, Any] | None = None,
    ) -> list[str]:
        """Starts as an ap using a set of configurations.

        This will start an ap on this host. To start an ap the controller
        selects a network interface to use based on the configs given. It then
        will start up hostapd on that interface. Next a subnet is created for
        the network interface and dhcp server is refreshed to give out ips
        for that subnet for any device that connects through that interface.

        Args:
            hostapd_config: The configurations to use when starting up the ap.
            radvd_config: The IPv6 configuration to use when starting up the ap.
            setup_bridge: Whether to bridge the LAN interface WLAN interface.
                Only one WLAN interface can be bridged with the LAN interface
                and none of the guest networks can be bridged.
            is_nat_enabled: If True, start NAT on the AP to allow the DUT to be
                able to access the internet if the WAN port is connected to the
                internet.
            additional_parameters: Parameters that can sent directly into the
                hostapd config file.  This can be used for debugging and or
                adding one off parameters into the config.

        Returns:
            An identifier for each ssid being started. These identifiers can be
            used later by this controller to control the ap.

        Raises:
            Error: When the ap can't be brought up.
        """
        if additional_parameters is None:
            additional_parameters = {}

        if hostapd_config.frequency < 5000:
            interface = self.wlan_2g
            subnet = self._AP_2G_SUBNET
        else:
            interface = self.wlan_5g
            subnet = self._AP_5G_SUBNET

        # radvd requires the interface to have a IPv6 link-local address.
        if radvd_config:
            self.ssh.run(f"sysctl -w net.ipv6.conf.{interface}.disable_ipv6=0")
            self.ssh.run(f"sysctl -w net.ipv6.conf.{interface}.forwarding=1")

        # In order to handle dhcp servers on any interface, the initiation of
        # the dhcp server must be done after the wlan interfaces are figured
        # out as opposed to being in __init__
        self._dhcp = DhcpServer(self.ssh, interface=interface)

        # For multi bssid configurations the mac address
        # of the wireless interface needs to have enough space to mask out
        # up to 8 different mac addresses. So in for one interface the range is
        # hex 0-7 and for the other the range is hex 8-f.
        ip = self.ssh.run(["ip", "link", "show", interface])

        # Example output:
        # 5: wlan0: <BROADCAST,MULTICAST> mtu 1500 qdisc mq state DOWN mode DEFAULT group default qlen 1000
        #     link/ether f4:f2:6d:aa:99:28 brd ff:ff:ff:ff:ff:ff

        lines = ip.stdout.decode("utf-8").splitlines()
        if len(lines) != 2:
            raise RuntimeError(
                f"Expected 2 lines from ip link show, got {len(lines)}"
            )
        tokens = lines[1].split()
        if len(tokens) != 4:
            raise RuntimeError(
                f"Expected 4 tokens from ip link show, got {len(tokens)}"
            )
        interface_mac_orig = tokens[1]

        if interface == self.wlan_5g:
            hostapd_config.bssid = f"{interface_mac_orig[:-1]}0"
            last_octet = 1
        elif interface == self.wlan_2g:
            hostapd_config.bssid = f"{interface_mac_orig[:-1]}8"
            last_octet = 9
        elif interface in self._aps:
            raise ValueError(
                "No WiFi interface available for AP on "
                f"channel {hostapd_config.channel}"
            )
        else:
            raise ValueError(f"Invalid WLAN interface: {interface}")

        apd = Hostapd(self.ssh, interface)
        new_instance = _ApInstance(hostapd=apd, subnet=subnet)
        self._aps[interface] = new_instance

        # Turn off the DHCP server, we're going to change its settings.
        self.stop_dhcp()
        # Clear all routes to prevent old routes from interfering.
        self._route_cmd.clear_routes(net_interface=interface)
        # Add IPv6 link-local route so packets destined to the AP will be
        # processed by the AP. This is necessary if an iperf server is running
        # on the AP, but not for traffic handled by the Linux networking stack
        # such as ping.
        if radvd_config:
            self._route_cmd.add_route(
                interface, ipaddress.IPv6Interface("fe80::/64")
            )

        self._dhcp_bss = dict()
        if hostapd_config.bss_lookup:
            # The self._dhcp_bss dictionary is created to hold the key/value
            # pair of the interface name and the ip scope that will be
            # used for the particular interface.  The a, b, c, d
            # variables below are the octets for the ip address.  The
            # third octet is then incremented for each interface that
            # is requested.  This part is designed to bring up the
            # hostapd interfaces and not the DHCP servers for each
            # interface.
            counter = 1
            for iface in hostapd_config.bss_lookup:
                hostapd_config.bss_lookup[iface].bssid = (
                    interface_mac_orig[:-1] + hex(last_octet)[-1:]
                )
                self._route_cmd.clear_routes(net_interface=str(iface))
                if interface is self.wlan_2g:
                    starting_ip_range = self._AP_2G_SUBNET_STR
                else:
                    starting_ip_range = self._AP_5G_SUBNET_STR
                a, b, c, d = starting_ip_range.split(".")
                self._dhcp_bss[iface] = Subnet(
                    ipaddress.IPv4Network(f"{a}.{b}.{int(c) + counter}.{d}")
                )
                counter = counter + 1
                last_octet = last_octet + 1

        apd.start(hostapd_config, additional_parameters=additional_parameters)

        # The DHCP serer requires interfaces to have ips and routes before
        # the server will come up.
        interface_ip = ipaddress.IPv4Interface(
            f"{subnet.router}/{subnet.network.prefixlen}"
        )
        bridge_interface_name = "eth_test"
        if setup_bridge is True:
            interfaces = [interface]
            if self.lan:
                interfaces.append(self.lan)
            self.create_bridge(bridge_interface_name, interfaces)
            self._ip_cmd.set_ipv4_address(bridge_interface_name, interface_ip)
        else:
            self._ip_cmd.set_ipv4_address(interface, interface_ip)
        if hostapd_config.bss_lookup:
            # This loop goes through each interface that was setup for
            # hostapd and assigns the DHCP scopes that were defined but
            # not used during the hostapd loop above.  The k and v
            # variables represent the interface name, k, and dhcp info, v.
            for iface, subnet in self._dhcp_bss.items():
                bss_interface_ip = ipaddress.IPv4Interface(
                    f"{subnet.router}/{subnet.network.prefixlen}"
                )
                self._ip_cmd.set_ipv4_address(iface, bss_interface_ip)

        # Restart the DHCP server with our updated list of subnets.
        configured_subnets = self.get_configured_subnets()
        dhcp_conf = DhcpConfig(subnets=configured_subnets)
        self.start_dhcp(dhcp_conf=dhcp_conf)
        if is_nat_enabled:
            self.start_nat()
            self.enable_forwarding()
        else:
            self.stop_nat()
            self.enable_forwarding()
        if radvd_config:
            radvd_interface = (
                bridge_interface_name if setup_bridge else interface
            )
            self._radvd = Radvd(self.ssh, radvd_interface)
            self._radvd.start(radvd_config)
        else:
            self._radvd = None

        bss_interfaces = [bss for bss in hostapd_config.bss_lookup]
        bss_interfaces.append(interface)

        return bss_interfaces

    def get_configured_subnets(self) -> list[Subnet]:
        """Get the list of configured subnets on the access point.

        This allows consumers of the access point objects create custom DHCP
        configs with the correct subnets.

        Returns: a list of Subnet objects
        """
        configured_subnets = [x.subnet for x in self._aps.values()]
        for k, v in self._dhcp_bss.items():
            configured_subnets.append(v)
        return configured_subnets

    def start_dhcp(self, dhcp_conf: DhcpConfig) -> None:
        """Start a DHCP server for the specified subnets.

        This allows consumers of the access point objects to control DHCP.

        Args:
            dhcp_conf: A DhcpConfig object.

        Raises:
            Error: Raised when a dhcp server error is found.
        """
        if self._dhcp is not None:
            self._dhcp.start(config=dhcp_conf)

    def stop_dhcp(self) -> None:
        """Stop DHCP for this AP object.

        This allows consumers of the access point objects to control DHCP.
        """
        if self._dhcp is not None:
            self._dhcp.stop()

    def get_systemd_journal(self) -> str:
        """Get systemd journal logs from this current boot."""
        return self._journalctl_cmd.logs()

    def get_dhcp_logs(self) -> str | None:
        """Get DHCP logs for this AP object.

        This allows consumers of the access point objects to validate DHCP
        behavior.

        Returns:
            A string of the dhcp server logs, or None is a DHCP server has not
            been started.
        """
        if self._dhcp is not None:
            return self._dhcp.get_logs()
        return None

    def get_hostapd_logs(self) -> dict[str, str]:
        """Get hostapd logs for all interfaces on AP object.

        This allows consumers of the access point objects to validate hostapd
        behavior.

        Returns: A dict with {interface: log} from hostapd instances.
        """
        hostapd_logs: dict[str, str] = dict()
        for iface, ap in self._aps.items():
            hostapd_logs[iface] = ap.hostapd.pull_logs()
        return hostapd_logs

    def get_radvd_logs(self) -> str | None:
        """Get radvd logs for this AP object.

        This allows consumers of the access point objects to validate radvd
        behavior.

        Returns:
            A string of the radvd logs, or None is a radvd server has not been
            started.
        """
        if self._radvd:
            return self._radvd.pull_logs()
        return None

    def download_ap_logs(self, path: str) -> None:
        """Download all available logs to path.

        This convenience method gets all the logs, dhcp, hostapd, radvd. It
        writes these to the given path.

        Args:
            path: Path to write logs to.
        """
        timestamp = logger.normalize_log_line_timestamp(
            logger.epoch_to_log_line_timestamp(utils.get_current_epoch_time())
        )

        dhcp_log = self.get_dhcp_logs()
        if dhcp_log:
            dhcp_log_path = os.path.join(path, f"ap_dhcp_{timestamp}.log")
            with open(dhcp_log_path, "a") as f:
                f.write(dhcp_log)
            self.log.debug(f"Wrote DHCP logs to {dhcp_log_path}")

        hostapd_logs = self.get_hostapd_logs()
        for interface in hostapd_logs:
            hostapd_log_path = os.path.join(
                path,
                f"ap_hostapd_{interface}_{timestamp}.log",
            )
            with open(hostapd_log_path, "a") as f:
                f.write(hostapd_logs[interface])
            self.log.debug(f"Wrote hostapd logs to {hostapd_log_path}")

        radvd_log = self.get_radvd_logs()
        if radvd_log:
            radvd_log_path = os.path.join(path, f"ap_radvd_{timestamp}.log")
            with open(radvd_log_path, "a") as f:
                f.write(radvd_log)
            self.log.debug(f"Wrote radvd logs to {radvd_log_path}")

        systemd_journal = self.get_systemd_journal()
        systemd_journal_path = os.path.join(path, f"ap_systemd_{timestamp}.log")
        with open(systemd_journal_path, "a") as f:
            f.write(systemd_journal)
        self.log.debug(f"Wrote systemd journal to {systemd_journal_path}")

    def enable_forwarding(self) -> None:
        """Enable IPv4 and IPv6 forwarding on the AP.

        When forwarding is enabled, the access point is able to route IP packets
        between devices in the same subnet.
        """
        self.ssh.run("echo 1 > /proc/sys/net/ipv4/ip_forward")
        self.ssh.run("echo 1 > /proc/sys/net/ipv6/conf/all/forwarding")

    def start_nat(self) -> None:
        """Start NAT on the AP.

        This allows consumers of the access point objects to enable NAT
        on the AP.

        Note that this is currently a global setting, since we don't
        have per-interface masquerade rules.
        """
        # The following three commands are needed to enable NAT between
        # the WAN and LAN/WLAN ports.  This means anyone connecting to the
        # WLAN/LAN ports will be able to access the internet if the WAN port
        # is connected to the internet.
        self.ssh.run("iptables -t nat -F")
        self.ssh.run(
            f"iptables -t nat -A POSTROUTING -o {self.wan} -j MASQUERADE"
        )

    def stop_nat(self) -> None:
        """Stop NAT on the AP.

        This allows consumers of the access point objects to disable NAT on the
        AP.

        Note that this is currently a global setting, since we don't have
        per-interface masquerade rules.
        """
        self.ssh.run("iptables -t nat -F")

    def create_bridge(self, bridge_name: str, interfaces: list[str]) -> None:
        """Create the specified bridge and bridge the specified interfaces.

        Args:
            bridge_name: The name of the bridge to create.
            interfaces: A list of interfaces to add to the bridge.
        """

        # Create the bridge interface
        self.ssh.run(f"brctl addbr {bridge_name}")

        for interface in interfaces:
            self.ssh.run(f"brctl addif {bridge_name} {interface}")

        self.ssh.run(f"ip link set {bridge_name} up")

    def remove_bridge(self, bridge_name: str) -> None:
        """Removes the specified bridge

        Args:
            bridge_name: The name of the bridge to remove.
        """
        # Check if the bridge exists.
        #
        # Cases where it may not are if we failed to initialize properly
        #
        # Or if we're doing 2.4Ghz and 5Ghz SSIDs and we've already torn
        # down the bridge once, but we got called for each band.
        result = self.ssh.run(f"brctl show {bridge_name}", ignore_status=True)

        # If the bridge exists, we'll get an exit_status of 0, indicating
        # success, so we can continue and remove the bridge.
        if result.returncode == 0:
            self.ssh.run(f"ip link set {bridge_name} down")
            self.ssh.run(f"brctl delbr {bridge_name}")

    def get_bssid_from_ssid(
        self, ssid: str, band: hostapd_constants.BandType
    ) -> MacAddress:
        """Gets the BSSID from a provided SSID

        Args:
            ssid: An SSID string.
            band: 2G or 5G Wifi band.

        Returns:
            The BSSID of on the AP hosting the given SSID on the given band.

        Raises:
            RuntimeError: when interface, ssid, or addr cannot be found.
        """
        match band:
            case hostapd_constants.BandType.BAND_2G:
                interface = self.wlan_2g
            case hostapd_constants.BandType.BAND_5G:
                interface = self.wlan_5g

        # Get the interface name associated with the given ssid.
        iw = self.ssh.run(["iw", "dev", interface, "info"])
        if b"command failed: No such device" in iw.stderr:
            raise RuntimeError(
                f'iw dev did not contain interface "{interface}"'
            )

        iw_out = iw.stdout.decode("utf-8")
        iw_lines = iw_out.splitlines()

        for line in iw_lines:
            if "ssid" in line and ssid in line:
                # Found the right interface.
                for line in iw_lines:
                    if "addr" in line:
                        tokens = line.split()
                        if len(tokens) != 2:
                            raise RuntimeError(
                                f"Expected iw dev info addr to have 2 tokens, got {tokens}"
                            )
                        return MacAddress(tokens[1])

                raise RuntimeError(
                    f"iw dev info contained ssid but not addr: \n{iw_out}"
                )

        raise RuntimeError(f'iw dev did not contain ssid "{ssid}"')

    def stop_ap(self, identifier: str) -> None:
        """Stops a running ap on this controller.

        Args:
            identifier: The identify of the ap that should be taken down.
        """

        instance = self._aps.get(identifier)
        if instance is None:
            raise ValueError(f"Invalid identifier {identifier} given")

        if self._radvd:
            self._radvd.stop()
        try:
            self.stop_dhcp()
        except NoInterfaceError:
            pass
        self.stop_nat()
        instance.hostapd.stop()
        self._ip_cmd.clear_ipv4_addresses(identifier)

        del self._aps[identifier]
        bridge_interfaces = self.interfaces.get_bridge_interface()
        for iface in bridge_interfaces:
            BRIDGE_DOWN = f"ip link set {iface} down"
            BRIDGE_DEL = f"brctl delbr {iface}"
            self.ssh.run(BRIDGE_DOWN)
            self.ssh.run(BRIDGE_DEL)

    def stop_all_aps(self) -> None:
        """Stops all running aps on this device."""

        for ap in list(self._aps.keys()):
            self.stop_ap(ap)

    def close(self) -> None:
        """Called to take down the entire access point.

        When called will stop all aps running on this host, shutdown the dhcp
        server, and stop the ssh connection.
        """

        if self._aps:
            self.stop_all_aps()
        self.ssh.close()

    def generate_bridge_configs(
        self, channel: int
    ) -> tuple[str, str | None, str]:
        """Generate a list of configs for a bridge between LAN and WLAN.

        Args:
            channel: the channel WLAN interface is brought up on
            iface_lan: the LAN interface to bridge
        Returns:
            configs: tuple containing iface_wlan, iface_lan and bridge_ip
        """

        if channel < 15:
            iface_wlan = self.wlan_2g
            subnet_str = self._AP_2G_SUBNET_STR
        else:
            iface_wlan = self.wlan_5g
            subnet_str = self._AP_5G_SUBNET_STR

        iface_lan = self.lan

        a, b, c, _ = subnet_str.strip("/24").split(".")
        bridge_ip = f"{a}.{b}.{c}.{BRIDGE_IP_LAST}"

        return (iface_wlan, iface_lan, bridge_ip)

    def ping(
        self,
        dest_ip: str,
        count: int = 3,
        interval: int = 1000,
        timeout: int = 1000,
        size: int = 56,
        additional_ping_params: str = "",
    ) -> utils.PingResult:
        """Pings from AP to dest_ip, returns dict of ping stats (see utils.ping)"""
        return utils.ping(
            self.ssh,
            dest_ip,
            count=count,
            interval=interval,
            timeout=timeout,
            size=size,
            additional_ping_params=additional_ping_params,
        )

    def hard_power_cycle(
        self,
        pdus: list[PduDevice],
    ) -> None:
        """Kills, then restores power to AccessPoint, verifying it goes down and
        comes back online cleanly.

        Args:
            pdus: PDUs in the testbed
        Raise:
            Error, if no PduDevice is provided in AccessPoint config.
            ConnectionError, if AccessPoint fails to go offline or come back.
        """
        if not self.device_pdu_config:
            raise Error("No PduDevice provided in AccessPoint config.")

        self._journalctl_cmd.save_and_reset()

        self.log.info("Power cycling")
        ap_pdu, ap_pdu_port = get_pdu_port_for_device(
            self.device_pdu_config, pdus
        )

        self.log.info("Killing power")
        ap_pdu.off(ap_pdu_port)

        self.log.info("Verifying AccessPoint is unreachable.")
        self.ssh_provider.wait_until_unreachable()
        self.log.info("AccessPoint is unreachable as expected.")

        self._aps.clear()

        self.log.info("Restoring power")
        ap_pdu.on(ap_pdu_port)

        self.log.info("Waiting for AccessPoint to become available via SSH.")
        self.ssh_provider.wait_until_reachable()
        self.log.info("AccessPoint responded to SSH.")

        # Allow 5 seconds for OS to finish getting set up
        time.sleep(5)
        self._initial_ap()
        self.log.info("Power cycled successfully")

    def channel_switch(
        self, identifier: str, channel_num: int, csa_beacon_count: int = 10
    ) -> None:
        """Switch to a different channel on the given AP."""
        instance = self._aps.get(identifier)
        if instance is None:
            raise ValueError(f"Invalid identifier {identifier} given")
        self.log.info(f"channel switch to channel {channel_num}")
        instance.hostapd.channel_switch(channel_num, csa_beacon_count)

    def get_current_channel(self, identifier: str) -> int:
        """Find the current channel on the given AP."""
        instance = self._aps.get(identifier)
        if instance is None:
            raise ValueError(f"Invalid identifier {identifier} given")
        return instance.hostapd.get_current_channel()

    def get_stas(self, identifier: str) -> set[MacAddress]:
        """Return MAC addresses of all associated STAs on the given AP."""
        instance = self._aps.get(identifier)
        if instance is None:
            raise ValueError(f"Invalid identifier {identifier} given")
        return instance.hostapd.get_stas()

    def sta_authenticated(self, identifier: str, sta_mac: MacAddress) -> bool:
        """Is STA authenticated?"""
        instance = self._aps.get(identifier)
        if instance is None:
            raise ValueError(f"Invalid identifier {identifier} given")
        return instance.hostapd.sta_authenticated(sta_mac)

    def sta_associated(self, identifier: str, sta_mac: MacAddress) -> bool:
        """Is STA associated?"""
        instance = self._aps.get(identifier)
        if instance is None:
            raise ValueError(f"Invalid identifier {identifier} given")
        return instance.hostapd.sta_associated(sta_mac)

    def sta_authorized(self, identifier: str, sta_mac: MacAddress) -> bool:
        """Is STA authorized (802.1X controlled port open)?"""
        instance = self._aps.get(identifier)
        if instance is None:
            raise ValueError(f"Invalid identifier {identifier} given")
        return instance.hostapd.sta_authorized(sta_mac)

    def get_sta_status(
        self,
        identifier_or_mac: str | MacAddress,
        sta_mac: MacAddress | None = None,
    ) -> StationStatus:
        """Get station status for the given STA, as seen by the AP.

        Args:
            identifier_or_mac: The AP interface identifier, or STA MAC if single arg.
            sta_mac: The STA MAC address if identifier was provided as first arg.
        """
        if sta_mac is None:
            sta_mac = MacAddress(str(identifier_or_mac))
            if not self._aps:
                raise ValueError("No AP instances running")
            for iface, inst in self._aps.items():
                try:
                    status = inst.hostapd.get_sta_status(sta_mac)
                except HostapdError:
                    continue
                if status.assoc:
                    if (
                        status.rssi is None
                        or status.tx_rate_mbps is None
                        or status.rx_rate_mbps is None
                    ):
                        raise ValueError(
                            f"Missing station telemetry on {iface} for {sta_mac}: {status}"
                        )
                    return status
            raise ValueError(
                f"Station {sta_mac} is not associated on any running AP instance"
            )

        identifier = str(identifier_or_mac)
        instance = self._aps.get(identifier)
        if instance is None:
            raise ValueError(f"Invalid identifier {identifier} given")
        status = instance.hostapd.get_sta_status(sta_mac)
        if (
            status.rssi is None
            or status.tx_rate_mbps is None
            or status.rx_rate_mbps is None
        ):
            raise ValueError(
                f"Missing station telemetry on {identifier} for {sta_mac}: {status}"
            )
        return status

    def get_sta_extended_capabilities(
        self, identifier: str, sta_mac: MacAddress
    ) -> ExtendedCapabilities:
        """Get extended capabilities for the given STA, as seen by the AP."""
        instance = self._aps.get(identifier)
        if instance is None:
            raise ValueError(f"Invalid identifier {identifier} given")
        return instance.hostapd.get_sta_extended_capabilities(sta_mac)

    def send_bss_transition_management_req(
        self,
        identifier: str,
        sta_mac: MacAddress,
        request: BssTransitionManagementRequest,
    ) -> None:
        """Send a BSS Transition Management request to an associated STA."""
        instance = self._aps.get(identifier)
        if instance is None:
            raise ValueError(f"Invalid identifier {identifier} given")
        instance.hostapd.send_bss_transition_management_req(sta_mac, request)


def setup_ap(
    access_point: AccessPoint,
    profile_name: str,
    channel: int,
    ssid: str,
    mode: str | None = None,
    preamble: bool | None = None,
    beacon_interval: int | None = None,
    dtim_period: int | None = None,
    frag_threshold: int | None = None,
    rts_threshold: int | None = None,
    force_wmm: bool | None = None,
    hidden: bool | None = False,
    security: Security | None = None,
    pmf_support: int | None = None,
    additional_ap_parameters: dict[str, Any] | None = None,
    n_capabilities: list[Any] | None = None,
    ac_capabilities: list[Any] | None = None,
    vht_bandwidth: int | None = None,
    wnm_features: FrozenSet[hostapd_constants.WnmFeature] = frozenset(),
    setup_bridge: bool = False,
    is_ipv6_enabled: bool = False,
    is_nat_enabled: bool = True,
    country: str | None = None,
) -> list[str]:
    """Creates a hostapd profile and runs it on an ap. This is a convenience
    function that allows us to start an ap with a single function, without first
    creating a hostapd config.

    Args:
        access_point: An ACTS access_point controller
        profile_name: The profile name of one of the hostapd ap presets.
        channel: What channel to set the AP to.
        preamble: Whether to set short or long preamble
        beacon_interval: The beacon interval
        dtim_period: Length of dtim period
        frag_threshold: Fragmentation threshold
        rts_threshold: RTS threshold
        force_wmm: Enable WMM or not
        hidden: Advertise the SSID or not
        security: What security to enable.
        pmf_support: Whether pmf is not disabled, enabled, or required
        additional_ap_parameters: Additional parameters to send the AP.
        check_connectivity: Whether to check for internet connectivity.
        wnm_features: WNM features to enable on the AP.
        setup_bridge: Whether to bridge the LAN interface WLAN interface.
            Only one WLAN interface can be bridged with the LAN interface
            and none of the guest networks can be bridged.
        is_ipv6_enabled: If True, start a IPv6 router advertisement daemon
        is_nat_enabled: If True, start NAT on the AP to allow the DUT to be able
            to access the internet if the WAN port is connected to the internet.
        country: the 2 char country code that the AP will operate with. The AP
            should be configured consistently with the regulatory domain's rules.

    Returns:
        An identifier for each ssid being started. These identifiers can be
        used later by this controller to control the ap.

    Raises:
        Error: When the ap can't be brought up.
    """
    if additional_ap_parameters is None:
        additional_ap_parameters = {}

    ap = create_ap_preset(
        profile_name=profile_name,
        iface_wlan_2g=access_point.wlan_2g,
        iface_wlan_5g=access_point.wlan_5g,
        channel=channel,
        ssid=ssid,
        mode=mode,
        short_preamble=preamble,
        beacon_interval=beacon_interval,
        dtim_period=dtim_period,
        frag_threshold=frag_threshold,
        rts_threshold=rts_threshold,
        force_wmm=force_wmm,
        hidden=hidden,
        bss_settings=[],
        security=security,
        pmf_support=pmf_support,
        n_capabilities=n_capabilities,
        ac_capabilities=ac_capabilities,
        vht_bandwidth=vht_bandwidth,
        wnm_features=wnm_features,
        country=country,
    )
    return access_point.start_ap(
        hostapd_config=ap,
        radvd_config=RadvdConfig() if is_ipv6_enabled else None,
        setup_bridge=setup_bridge,
        is_nat_enabled=is_nat_enabled,
        additional_parameters=additional_ap_parameters,
    )
