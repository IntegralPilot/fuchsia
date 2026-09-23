# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import ipaddress
import re
import subprocess
from typing import Iterator

from libs.commands.command import LinuxCommand
from libs.proc.runner import Runner
from mobly import signals


class LinuxIpCommand(LinuxCommand):
    """Interface for doing standard IP commands on a linux system.

    Wraps standard shell commands used for ip into a python object that can
    be interacted with more easily.
    """

    def __init__(self, runner: Runner, binary: str = "ip"):
        """Create a LinuxIpCommand.

        Args:
            runner: Runner to use to execute this command.
            binary: Path to binary to use. Defaults to "ip".
            sudo: Requires root permissions. Defaults to False.
        """
        super().__init__(runner, binary)

    def get_ipv4_addresses(
        self, net_interface: str
    ) -> Iterator[tuple[ipaddress.IPv4Interface, ipaddress.IPv4Address | None]]:
        """Gets all ipv4 addresses of a network interface.

        Args:
            net_interface: string, The network interface to get info on
                           (eg. wlan0).

        Returns: An iterator of tuples that contain (address, broadcast).
                 where address is a ipaddress.IPv4Interface and broadcast
                 is an ipaddress.IPv4Address.
        """
        results = self._run(["addr", "show", "dev", net_interface])
        lines = results.stdout.splitlines()

        # Example stdout:
        # 2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc mq state UP group default qlen 1000
        #   link/ether 48:0f:cf:3c:9d:89 brd ff:ff:ff:ff:ff:ff
        #   inet 192.168.1.1/24 brd 192.168.1.255 scope global eth0
        #       valid_lft forever preferred_lft forever
        #   inet6 2620:0:1000:1500:a968:a776:2d80:a8b3/64 scope global temporary dynamic
        #       valid_lft 599919sec preferred_lft 80919sec

        for line_bytes in lines:
            line = line_bytes.decode("utf-8").strip()
            match = re.search(
                "inet (?P<address>[^\\s]*) brd (?P<bcast>[^\\s]*)", line
            )
            if match:
                d = match.groupdict()
                address = ipaddress.IPv4Interface(d["address"])
                bcast = ipaddress.IPv4Address(d["bcast"])
                yield (address, bcast)

            match = re.search("inet (?P<address>[^\\s]*)", line)
            if match:
                d = match.groupdict()
                address = ipaddress.IPv4Interface(d["address"])
                yield (address, None)

    def add_ipv4_address(
        self,
        net_interface: str,
        address: ipaddress.IPv4Interface,
        broadcast: ipaddress.IPv4Address | None = None,
    ) -> None:
        """Adds an ipv4 address to a net_interface.

        Args:
            net_interface: The network interface to get the new ipv4 (eg. wlan0).
            address: The new ipaddress and netmask to add to an interface.
            broadcast: The broadcast address to use for this net_interfaces subnet.
        """
        args = ["addr", "add", str(address)]
        if broadcast:
            args += ["broadcast", str(broadcast)]
        args += ["dev", net_interface]
        self._run(args, sudo=True)

    def remove_ipv4_address(
        self,
        net_interface: str,
        address: ipaddress.IPv4Interface | ipaddress.IPv4Address,
        ignore_status: bool = False,
    ) -> None:
        """Remove an ipv4 address.

        Removes an ipv4 address from a network interface.

        Args:
            net_interface: The network interface to remove the ipv4 address from (eg. wlan0).
            address: The ip address to remove from the net_interface.
            ignore_status: True if the exit status can be ignored
        Returns:
            The job result from a the command
        """
        try:
            self._run(
                ["addr", "del", str(address), "dev", net_interface],
                sudo=True,
            )
        except subprocess.CalledProcessError as e:
            if e.returncode == 2 or b"Address not found" in e.stdout:
                # Do not fail if the address was already deleted or couldn't be
                # found.
                return
            raise e

    def set_ipv4_address(
        self,
        net_interface: str,
        address: ipaddress.IPv4Interface,
        broadcast: ipaddress.IPv4Address | None = None,
    ) -> None:
        """Set the ipv4 address.

        Sets the ipv4 address of a network interface. If the network interface
        has any other ipv4 addresses these will be cleared.

        Args:
            net_interface: The network interface to set the ip address on (eg. wlan0).
            address: The ip address and subnet to give the net_interface.
            broadcast: The broadcast address to use for the subnet.
        """
        self.clear_ipv4_addresses(net_interface)
        self.add_ipv4_address(net_interface, address, broadcast)

    def clear_ipv4_addresses(self, net_interface: str) -> None:
        """Clears all ipv4 addresses registered to a net_interface.

        Args:
            net_interface: The network interface to clear addresses from (eg. wlan0).
        """
        ip_info = self.get_ipv4_addresses(net_interface)

        for address, _ in ip_info:
            try:
                self.remove_ipv4_address(net_interface, address)
            except subprocess.CalledProcessError as e:
                if (
                    b"RTNETLINK answers: Cannot assign requested address"
                    in e.stderr
                ):
                    # It is possible that the address has already been removed by the
                    # time this command has been called.
                    addresses = [
                        a for a, _ in self.get_ipv4_addresses(net_interface)
                    ]
                    if address not in addresses:
                        self._runner.log.warning(
                            "Unable to remove address %s. The address was "
                            "removed by another process.",
                            address,
                        )
                    else:
                        raise signals.TestError(
                            f"Unable to remove address {address}. The address is still "
                            f"registered to {net_interface}, despite call for removal.",
                            extras={
                                "stderr": e.stderr,
                                "stdout": e.stdout,
                                "returncode": e.returncode,
                            },
                        )
                raise signals.TestError(
                    f"Unable to remove address {address}: {e.stderr.decode('utf-8', errors='replace')}",
                    extras={
                        "stdout": e.stdout,
                        "returncode": e.returncode,
                    },
                )
