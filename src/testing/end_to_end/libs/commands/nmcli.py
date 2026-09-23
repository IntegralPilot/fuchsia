# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import enum
from dataclasses import dataclass

from libs.commands import pgrep
from libs.commands.command import LinuxCommand
from libs.proc.runner import Runner


class LinuxNmcliCommand(LinuxCommand):
    """Control the Linux NetworkManager.

    The NetworkManager daemon attempts to make networking configuration and
    operation as painless and automatic as possible by managing the primary
    network connection and other network interfaces, like Ethernet, Wi-Fi, and
    Mobile Broadband devices. NetworkManager will connect any network device
    when a connection for that device becomes available, unless that behavior is
    disabled.
    """

    def __init__(self, runner: Runner, binary: str = "nmcli") -> None:
        super().__init__(runner, binary)
        self._pgrep = pgrep.LinuxPgrepCommand(runner)

    def available(self) -> bool:
        if not super().available():
            return False
        if not self._pgrep.available():
            return False
        return self._pgrep.find("NetworkManager") is not None

    def setup_device(self, device: str) -> None:
        """Create a device connection suitable for antlion testing.

        Disables IPv4 DHCP so that tests can manage IP addresses manually, but
        still enables automatic IPv6 link-local address assignment.
        """
        # Remove existing connections associated with device.
        for conn in self._get_connections():
            if conn.device == device:
                self._delete_connection(conn)

        self._run(
            [
                "connection",
                "add",
                "ifname",
                device,
                "type",
                "ethernet",
                "ipv4.method",
                IPv4Method.DISABLED,
                "ipv6.method",
                IPv6Method.LINK_LOCAL,
            ],
            sudo=True,
        )

    def _get_connections(self) -> list[Connection]:
        res = self._run(
            [
                "--get-values",
                "name,uuid,type,device",
                "connection",
            ],
            sudo=True,
        )
        connections: list[Connection] = []
        for line in res.stdout.splitlines():
            tokens = line.decode("utf-8").split(":", 3)
            connections.append(
                Connection(
                    name=tokens[0],
                    uuid=tokens[1],
                    type=tokens[2],
                    device=tokens[3],
                )
            )
        return connections

    def _delete_connection(self, conn: Connection) -> None:
        self._run(
            [
                "connection",
                "delete",
                "id",
                conn.name,
            ],
            sudo=True,
        )

    def _down_device(self, device: str) -> None:
        self._run(
            [
                "device",
                "down",
                device,
            ],
            sudo=True,
        )

    def _up_device(self, device: str) -> None:
        self._run(
            [
                "device",
                "up",
                device,
            ],
            sudo=True,
        )

    def set_ipv4_method(self, device: str, method: IPv4Method) -> None:
        """Set the IPv4 connection method.

        Args:
            device: Name of the device to modify.
            method: Connection method to use.
        """
        self._run(
            [
                "device",
                "modify",
                device,
                "ipv4.method",
                method,
            ],
            sudo=True,
        )


@dataclass(frozen=True)
class Connection:
    name: str
    uuid: str
    type: str
    device: str


class IPv4Method(enum.StrEnum):
    AUTO = "auto"
    """Enables automatic IPv4 address assignment from DHCP, PPP, or similar services."""

    MANUAL = "manual"
    """Enables the configuration of static IPv4 addresses on the interface.

    Note that you must set at least one IP address and subnet mask in the
    "ipv4.addresses" property.
    """

    DISABLED = "disabled"
    """Disables the IPv4 protocol in this connection profile."""

    SHARED = "shared"
    """Provides network access to other computers.

    If you do not specify an IP address and subnet mask in "ipv4.addresses",
    NetworkManager assigns 10.42.x.1/24 to the interface. Additionally,
    NetworkManager starts a DHCP server and DNS forwarder. Hosts that connect to
    this interface will then receive an IP address from the configured range,
    and NetworkManager configures NAT to map client addresses to the one of the
    current default network connection.
    """

    LINK_LOCAL = "link-local"
    """Enables link-local addresses according to RFC 3927.

    NetworkManager assigns a random link-local address from the 169.254.0.0/16
    subnet to the interface.
    """


class IPv6Method(enum.StrEnum):
    AUTO = "auto"
    """Enables IPv6 auto-configuration.

    By default, NetworkManager uses Router Advertisements and, if the router
    announces the "managed" flag, NetworkManager requests an IPv6 address and
    prefix from a DHCPv6 server.
    """

    DHCP = "dhcp"
    """Requests an IPv6 address and prefix from a DHCPv6 server.

    Note that DHCPv6 does not have options to provide routes and the default
    gateway. As a consequence, by using the "dhcp" method, connections are
    limited to their own subnet.
    """

    MANUAL = "manual"
    """Enables the configuration of static IPv6 addresses on the interface.

    Note that you must set at least one IP address and prefix in the
    "ipv6.addresses" property.
    """

    DISABLED = "disabled"
    """Disables the IPv6 protocol in this connection profile."""

    IGNORE = "ignore"
    """Make no changes to the IPv6 configuration on the interface.

    For example, you can then use the "accept_ra" feature of the kernel to
    accept Router Advertisements.
    """

    SHARED = "shared"
    """Provides network access to other computers.

    NetworkManager requests a prefix from an upstream DHCPv6 server, assigns an
    address to the interface, and announces the prefix to clients that connect
    to this interface.
    """

    LINK_LOCAL = "link-local"
    """Enabled link-local addresses according to RFC 3927.

    Assigns a random link-local address from the fe80::/64 subnet to the
    interface.
    """
