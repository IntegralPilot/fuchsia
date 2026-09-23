# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import copy
import ipaddress
from ipaddress import IPv4Address, IPv4Network

_ROUTER_DNS = "8.8.8.8, 4.4.4.4"


class Subnet(object):
    """Configs for a subnet  on the dhcp server.

    Attributes:
        network: ipaddress.IPv4Network, the network that this subnet is in.
        start: ipaddress.IPv4Address, the start ip address.
        end: ipaddress.IPv4Address, the end ip address.
        router: The router to give to all hosts in this subnet.
        lease_time: The lease time of all hosts in this subnet.
        additional_parameters: A dictionary corresponding to DHCP parameters.
        additional_options: A dictionary corresponding to DHCP options.
    """

    def __init__(
        self,
        subnet: IPv4Network,
        start: IPv4Address | None = None,
        end: IPv4Address | None = None,
        router: IPv4Address | None = None,
        lease_time: int | None = None,
        additional_parameters: dict[str, str] = {},
        additional_options: dict[str, int | str] = {},
    ):
        """
        Args:
            subnet: ipaddress.IPv4Network, The address space of the subnetwork
                    served by the DHCP server.
            start: ipaddress.IPv4Address, The start of the address range to
                   give hosts in this subnet. If not given, the second ip in
                   the network is used, under the assumption that the first
                   address is the router.
            end: ipaddress.IPv4Address, The end of the address range to give
                 hosts. If not given then the address prior to the broadcast
                 address (i.e. the second to last ip in the network) is used.
            router: ipaddress.IPv4Address, The router hosts should use in this
                    subnet. If not given the first ip in the network is used.
            lease_time: int, The amount of lease time in seconds
                        hosts in this subnet have.
            additional_parameters: A dictionary corresponding to DHCP parameters.
            additional_options: A dictionary corresponding to DHCP options.
        """
        self.network = subnet

        if start:
            self.start = start
        else:
            self.start = self.network[2]

        if not self.start in self.network:
            raise ValueError("The start range is not in the subnet.")
        if self.start.is_reserved:
            raise ValueError("The start of the range cannot be reserved.")

        if end:
            self.end = end
        else:
            self.end = self.network[-2]

        if not self.end in self.network:
            raise ValueError("The end range is not in the subnet.")
        if self.end.is_reserved:
            raise ValueError("The end of the range cannot be reserved.")
        if self.end < self.start:
            raise ValueError(
                "The end must be an address larger than the start."
            )

        if router:
            if router >= self.start and router <= self.end:
                raise ValueError("Router must not be in pool range.")
            if not router in self.network:
                raise ValueError("Router must be in the given subnet.")

            self.router = router
        else:
            # TODO: Use some more clever logic so that we don't have to search
            # every host potentially.
            # This is especially important if we support IPv6 networks in this
            # configuration. The improved logic that we can use is:
            #    a) erroring out if start and end encompass the whole network, and
            #    b) picking any address before self.start or after self.end.
            for host in self.network.hosts():
                if host < self.start or host > self.end:
                    self.router = host
                    break

            if not hasattr(self, "router"):
                raise ValueError("No useable host found.")

        self.lease_time = lease_time
        self.additional_parameters = additional_parameters
        self.additional_options = additional_options
        if "domain-name-servers" not in self.additional_options:
            self.additional_options["domain-name-servers"] = _ROUTER_DNS


class StaticMapping(object):
    """Represents a static dhcp host.

    Attributes:
        identifier: How id of the host (usually the mac addres
                    e.g. 00:11:22:33:44:55).
        address: ipaddress.IPv4Address, The ipv4 address to give the host.
        lease_time: How long to give a lease to this host.
    """

    def __init__(
        self,
        identifier: str,
        address: ipaddress.IPv4Address,
        lease_time: int | None = None,
    ) -> None:
        self.identifier = identifier
        self.ipv4_address = address
        self.lease_time = lease_time


class DhcpConfig(object):
    """The configs for a dhcp server.

    Attributes:
        subnets: A list of all subnets for the dhcp server to create.
        static_mappings: A list of static host addresses.
        default_lease_time: The default time for a lease.
        max_lease_time: The max time to allow a lease.
    """

    def __init__(
        self,
        subnets: list[Subnet] | None = None,
        static_mappings: list[StaticMapping] | None = None,
        default_lease_time: int = 600,
        max_lease_time: int = 7200,
    ) -> None:
        self.subnets = copy.deepcopy(subnets) if subnets else []
        self.static_mappings = (
            copy.deepcopy(static_mappings) if static_mappings else []
        )
        self.default_lease_time = default_lease_time
        self.max_lease_time = max_lease_time

    def render_config_file(self) -> str:
        """Renders the config parameters into a format compatible with
        the ISC DHCP server (dhcpd).
        """
        lines = []

        if self.default_lease_time:
            lines.append(f"default-lease-time {self.default_lease_time};")
        if self.max_lease_time:
            lines.append(f"max-lease-time {self.max_lease_time};")

        for subnet in self.subnets:
            address = subnet.network.network_address
            mask = subnet.network.netmask
            router = subnet.router
            start = subnet.start
            end = subnet.end
            lease_time = subnet.lease_time
            additional_parameters = subnet.additional_parameters
            additional_options = subnet.additional_options

            lines.append("subnet %s netmask %s {" % (address, mask))
            lines.append("\tpool {")
            lines.append(f"\t\toption subnet-mask {mask};")
            lines.append(f"\t\toption routers {router};")
            lines.append(f"\t\trange {start} {end};")
            if lease_time:
                lines.append(f"\t\tdefault-lease-time {lease_time};")
                lines.append(f"\t\tmax-lease-time {lease_time};")
            for param, value in additional_parameters.items():
                lines.append(f"\t\t{param} {value};")
            for option, option_value in additional_options.items():
                lines.append(f"\t\toption {option} {option_value};")
            lines.append("\t}")
            lines.append("}")

        for mapping in self.static_mappings:
            identifier = mapping.identifier
            fixed_address = mapping.ipv4_address
            host_fake_name = f"host{identifier.replace(':', '')}"
            lease_time = mapping.lease_time

            lines.append("host %s {" % host_fake_name)
            lines.append(f"\thardware ethernet {identifier};")
            lines.append(f"\tfixed-address {fixed_address};")
            if lease_time:
                lines.append(f"\tdefault-lease-time {lease_time};")
                lines.append(f"\tmax-lease-time {lease_time};")
            lines.append("}")

        config_str = "\n".join(lines)

        return config_str
