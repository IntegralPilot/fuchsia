# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import ipaddress
import re
import subprocess
from typing import Iterator, Literal

from libs.commands.command import LinuxCommand
from libs.proc.runner import Runner
from mobly import signals


class Error(Exception):
    """Exception thrown when a valid ip command experiences errors."""


class LinuxRouteCommand(LinuxCommand):
    """Interface for doing standard ip route commands on a linux system."""

    def __init__(self, runner: Runner, binary: str = "ip"):
        super().__init__(runner, binary)

    def add_route(
        self,
        net_interface: str,
        address: ipaddress.IPv4Interface
        | ipaddress.IPv6Interface
        | Literal["default"],
        proto: str = "static",
    ) -> None:
        """Add an entry to the ip routing table.

        Will add a route for either a specific ip address, or a network.

        Args:
            net_interface: Any packet that sends through this route will be sent
                using this network interface (eg. wlan0).
            address: The address to use. If a network is given then the entire
                subnet will be routed. If "default" is given then this will set the
                default route.
            proto: Routing protocol identifier of this route (e.g. kernel,
                redirect, boot, static, ra). See `man ip-route(8)` for details.

        Raises:
            NetworkInterfaceDown: Raised when the network interface is down.
        """
        try:
            self._run(
                [
                    "route",
                    "add",
                    str(address),
                    "dev",
                    net_interface,
                    "proto",
                    proto,
                ],
                sudo=True,
            )
        except subprocess.CalledProcessError as e:
            if b"File exists" in e.stderr:
                raise signals.TestError(
                    "Route already exists",
                    extras={
                        "stderr": e.stderr,
                        "stdout": e.stdout,
                        "returncode": e.returncode,
                    },
                )
            if b"Network is down" in e.stderr:
                raise signals.TestError(
                    "Device must be up for adding a route.",
                    extras={
                        "stderr": e.stderr,
                        "stdout": e.stdout,
                        "returncode": e.returncode,
                    },
                )
            raise e

    def get_routes(
        self, net_interface: str | None = None
    ) -> Iterator[
        tuple[
            ipaddress.IPv4Interface
            | ipaddress.IPv6Interface
            | Literal["default"],
            str,
        ]
    ]:
        """Get the routes in the ip routing table.

        Args:
            net_interface: string, If given, only retrieve routes that have
                           been registered to go through this network
                           interface (eg. wlan0).

        Returns: An iterator that returns a tuple of (address, net_interface).
                 If it is the default route then address
                 will be the "default". If the route is a subnet then
                 it will be a ipaddress.IPv4Network otherwise it is a
                 ipaddress.IPv4Address.
        """
        result_ipv4 = self._run(["-4", "route", "show"])
        result_ipv6 = self._run(["-6", "route", "show"])

        lines = (
            result_ipv4.stdout.splitlines() + result_ipv6.stdout.splitlines()
        )

        # Scan through each line for valid route entries
        # Example output:
        # default via 192.168.1.254 dev eth0  proto static
        # 192.168.1.0/24 dev eth0  proto kernel  scope link  src 172.22.100.19  metric 1
        # 192.168.2.1 dev eth2 proto kernel scope link metric 1
        # fe80::/64 dev wlan0 proto static metric 1024
        for line_bytes in lines:
            line = line_bytes.decode("utf-8")
            if not "dev" in line:
                continue

            if line.startswith("default"):
                # The default route entry is formatted differently.
                match = re.search("dev (?P<net_interface>\\S+)", line)
                if not match:
                    continue

                iface = match.groupdict()["net_interface"]
                assert isinstance(iface, str)

                if net_interface and iface != net_interface:
                    continue

                # When there is a match for the route entry pattern create
                # A pair to hold the info.
                yield ("default", iface)
            else:
                # Test the normal route entry pattern.
                match = re.search(
                    "(?P<address>[0-9A-Fa-f\\.\\:/]+) dev (?P<net_interface>\\S+)",
                    line,
                )
                if not match:
                    continue

                # When there is a match for the route entry pattern create
                # A pair to hold the info.
                d = match.groupdict()

                address_raw = d["address"]
                assert isinstance(address_raw, str)

                iface = d["net_interface"]
                assert isinstance(iface, str)

                if net_interface and iface != net_interface:
                    continue

                yield (ipaddress.ip_interface(address_raw), iface)

    def remove_route(
        self,
        address: ipaddress.IPv4Interface
        | ipaddress.IPv6Interface
        | Literal["default"],
        net_interface: str | None = None,
    ) -> None:
        """Removes a route from the ip routing table.

        Removes a route from the ip routing table. If the route does not exist
        nothing is done.

        Args:
            address: The address of the route to remove.
            net_interface: If specified the route being removed is registered to
                go through this network interface (eg. wlan0)
        """
        try:
            args = ["route", "del", str(address)]
            if net_interface:
                args += ["dev", net_interface]
            self._run(args)
        except subprocess.CalledProcessError as e:
            if b"RTNETLINK answers: No such process" in e.stderr:
                # The route didn't exist.
                return
            raise signals.TestError(
                f"Failed to delete route {address}: {e}"
            ) from e

    def clear_routes(self, net_interface: str) -> None:
        """Clears all routes.

        Args:
            net_interface: The network interface to clear routes on.
        """
        routes = self.get_routes(net_interface)
        for a, d in routes:
            self.remove_route(a, d)
