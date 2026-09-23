# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Data types used by netstack affordance."""

from __future__ import annotations

import enum
from dataclasses import dataclass
from ipaddress import IPv4Address, IPv6Address

import fidl_fuchsia_net as f_net
import fidl_fuchsia_net_interfaces as f_net_interfaces
from honeydew.typing.custom_types import MacAddress


# Not all fields of fuchsia.net.interfaces/Properties are added below.
#
# TODO(http://b/355718339): Replace with statically generated FIDL Python type
# once available. See fuchsia.net.interfaces/Properties to view additional
# fields that may be implemented.
@dataclass
class InterfaceProperties:
    """Properties of a network interface."""

    id_: int
    """An opaque identifier for the interface."""

    name: str
    """The name of the interface."""

    mac: MacAddress | None
    """The MAC address of the interface, if there is one."""

    ipv4_addresses: list[IPv4Address]
    """IPv4 addresses currently installed on the interface."""

    ipv6_addresses: list[IPv6Address]
    """IPv6 addresses currently installed on the interface."""

    port_class: PortClass
    """The port class of the interface."""

    @staticmethod
    def from_fidl(
        fidl: f_net_interfaces.Properties, mac: MacAddress | None
    ) -> InterfaceProperties:
        """Create an InterfaceProperties from the FIDL equivalent.

        `mac` is necessary since fuchsia.net.interface/Properties does not
        include a MAC address. It has to be fetched separately using
        fuchsia.net.root/Interfaces.GetMac()
        """
        ipv4_addresses: list[IPv4Address] = []
        ipv6_addresses: list[IPv6Address] = []

        assert fidl.addresses is not None, f"{fidl!r} missing addresses"
        assert fidl.port_class is not None, f"{fidl!r} missing port_class"
        assert fidl.id_ is not None, f"{fidl!r} missing id"
        assert fidl.name is not None, f"{fidl!r} missing name"

        for address in fidl.addresses:
            assert address.addr is not None, f"{address!r} missing addr"
            subnet = address.addr
            ip = subnet.addr
            if ip.ipv4:
                ipv4_addresses.append(IPv4Address(bytes(ip.ipv4.addr)))
            elif ip.ipv6:
                ipv6_addresses.append(IPv6Address(bytes(ip.ipv6.addr)))
            else:
                raise TypeError(f"Unknown IP address type: {ip}")

        if fidl.port_class.loopback:
            port_class = PortClass.LOOPBACK
        elif fidl.port_class.device:
            port_class = PortClass(fidl.port_class.device)
        elif fidl.port_class.blackhole:
            port_class = PortClass.BLACKHOLE
        else:
            raise TypeError(f"Unknown port_class: {fidl.port_class}")

        return InterfaceProperties(
            id_=fidl.id_,
            name=fidl.name,
            mac=mac,
            ipv4_addresses=ipv4_addresses,
            ipv6_addresses=ipv6_addresses,
            port_class=port_class,
        )

    def to_fidl(self) -> f_net_interfaces.Properties:
        """Convert to the FIDL equivalent."""
        addresses: list[f_net_interfaces.Address] = []

        for ipv4 in self.ipv4_addresses:
            addr = f_net.IpAddress(
                ipv4=f_net.Ipv4Address(
                    addr=list(ipv4.packed),
                )
            )
            addresses.append(
                f_net_interfaces.Address(
                    addr=f_net.Subnet(
                        addr=addr,
                        prefix_len=0,
                    ),
                    valid_until=None,
                    preferred_lifetime_info=None,
                    assignment_state=None,
                )
            )

        for ipv6 in self.ipv6_addresses:
            addr = f_net.IpAddress(
                ipv6=f_net.Ipv6Address(
                    addr=list(ipv6.packed),
                )
            )
            addresses.append(
                f_net_interfaces.Address(
                    addr=f_net.Subnet(
                        addr=addr,
                        prefix_len=0,
                    ),
                    valid_until=None,
                    preferred_lifetime_info=None,
                    assignment_state=None,
                )
            )

        if self.port_class is PortClass.LOOPBACK:
            port_class = f_net_interfaces.PortClass(
                loopback=f_net_interfaces.Empty()
            )
        elif self.port_class is PortClass.BLACKHOLE:
            port_class = f_net_interfaces.PortClass(
                blackhole=f_net_interfaces.Empty()
            )
        else:
            port_class = f_net_interfaces.PortClass(
                device=self.port_class.value
            )

        return f_net_interfaces.Properties(
            id_=self.id_,
            addresses=addresses,
            online=None,
            has_default_ipv4_route=None,
            has_default_ipv6_route=None,
            name=self.name,
            port_class=port_class,
        )


class PortClass(enum.IntEnum):
    """Network port class.

    Loosely mirrors fuchsia.hardware.network/PortClass.
    """

    LOOPBACK = 0  # not part of fuchsia.hardware.network/PortClass
    ETHERNET = 1
    WLAN_CLIENT = 2
    PPP = 3
    BRIDGE = 4
    WLAN_AP = 5
    VIRTUAL = 6
    LOWPAN = 7
    BLACKHOLE = 8  # not part of fuchsia.hardware.network/PortClass


@dataclass
class PingResult:
    """Result of a ping operation."""

    raw_output: str  # Raw stdout and stderr from ping command.
    requested: int
    transmitted: int
    received: int
    time_ms: float | None
    rtt_min_ms: float | None
    rtt_avg_ms: float | None
    rtt_max_ms: float | None
    rtt_mdev_ms: float | None

    @property
    def all_pings_received(self) -> bool:
        """True if all requested pings were received."""
        return (
            self.received == self.requested
            and self.transmitted == self.requested
        )

    @property
    def any_pings_received(self) -> bool:
        """True if at least one ping was received."""
        return self.received > 0
