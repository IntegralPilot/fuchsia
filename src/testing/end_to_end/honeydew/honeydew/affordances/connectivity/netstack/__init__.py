# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Netstack affordance implementation using Fuchsia Controller."""

from __future__ import annotations

import asyncio
import enum
import logging
import re
import subprocess
import time
from datetime import timedelta
from ipaddress import IPv4Address, IPv6Address

import fidl_fuchsia_net_interfaces as f_net_interfaces
import fidl_fuchsia_net_root as f_net_root
from fuchsia_controller_py import FcTransportStatus, ZxStatus
from honeydew import affordances_capable
from honeydew import errors as honeydew_errors
from honeydew.affordances.connectivity.netstack.errors import (
    HoneydewNetstackError,
)
from honeydew.affordances.connectivity.netstack.types import (
    InterfaceProperties,
    PingResult,
    PortClass,
)
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.ffx import types as ffx_types
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing.custom_types import FidlEndpoint, MacAddress

# List of required FIDLs for this affordance.
_REQUIRED_CAPABILITIES = [
    "fuchsia.net.interfaces.State",
]

_LOGGER: logging.Logger = logging.getLogger(__name__)

# Fuchsia Controller proxies
_STATE_PROXY = FidlEndpoint(
    "core/network/netstack", "fuchsia.net.interfaces.State"
)
_INTERFACES_PROXY = FidlEndpoint(
    "core/network/netstack", "fuchsia.net.root.Interfaces"
)


class Netstack:
    """Netstack affordance implemented with Fuchsia Controller."""

    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
    ) -> None:
        """Create a Netstack Fuchsia Controller affordance.

        Args:
            device_name: Device name returned by `ffx target list`.
            ffx: FFX transport.
            fuchsia_controller: Fuchsia Controller transport.
            reboot_affordance: Object that implements RebootCapableDevice.
        """
        super().__init__()

        self._fc_transport = fuchsia_controller
        self._reboot_affordance = reboot_affordance
        self.device = device_name
        self.ffx = ffx

        self.verify_supported()

        self._connect_proxy()
        self._reboot_affordance.register_for_on_device_boot(self._connect_proxy)

    def verify_supported(self) -> None:
        """Check if Netstack is supported on the DUT.

        Raises:
            NotSupportedError: Netstack affordance is not supported by Fuchsia device.
        """
        for capability in _REQUIRED_CAPABILITIES:
            # TODO(http://b/359342196): This is a maintenance burden; find a
            # better way to detect FIDL component capabilities.
            if capability not in self.ffx.run(
                ["component", "capability", capability],
                # TODO(b/474143046) update to JSON when ffx supports it
                machine=ffx_types.MachineFormat.RAW,
            ):
                _LOGGER.warning(
                    "All available netstack component capabilities:\n%s",
                    self.ffx.run(
                        ["component", "capability", "fuchsia.net"],
                        # TODO(b/474143046) update to JSON when ffx supports it
                        machine=ffx_types.MachineFormat.RAW,
                    ),
                )
                raise honeydew_errors.NotSupportedError(
                    f'Component capability "{capability}" not exposed by device '
                    f"{self.device}; this build of Fuchsia does not support the "
                    "Netstack FC affordance."
                )

    def _connect_proxy(self) -> None:
        """Re-initializes connection to the Netstack."""
        self._state_proxy = f_net_interfaces.StateClient(
            self._fc_transport.connect_device_proxy(_STATE_PROXY)
        )
        self._interfaces_proxy = f_net_root.InterfacesClient(
            self._fc_transport.connect_device_proxy(_INTERFACES_PROXY)
        )

    async def list_interfaces(self) -> list[InterfaceProperties]:
        """List interfaces.

        Returns:
            Information on all interfaces on the device.

        Raises:
            HoneydewNetstackError: Error from the netstack.
            TypeError: Received invalid Watcher events from netstack.
        """
        client, server = self._fc_transport.channel_create()
        watcher = f_net_interfaces.WatcherClient(client.take())

        try:
            self._state_proxy.get_watcher(
                options=f_net_interfaces.WatcherOptions(
                    address_properties_interest=None,
                    # When an IP address is undergoing DAD, it cannot yet be used.
                    # Ask netstack to avoid reporting the address until it is
                    # actually ready to use.
                    include_non_assigned_addresses=False,
                ),
                watcher=server.take(),
            )
        except FcTransportStatus as status:
            raise HoneydewNetstackError(
                f"State.GetWatcher() error {status}"
            ) from status

        properties: list[InterfaceProperties] = []

        while True:
            try:
                resp = await watcher.watch()
            except FcTransportStatus as status:
                raise HoneydewNetstackError(
                    f"Watcher.Watch() error {status}"
                ) from status

            event = resp.event
            if event.existing:
                assert (
                    event.existing.id_ is not None
                ), f"{event.existing!r} missing id"
                try:
                    try:
                        get_mac_response = (
                            await self._interfaces_proxy.get_mac(
                                id_=event.existing.id_
                            )
                        ).unwrap()
                    except AssertionError:
                        _LOGGER.debug(
                            'Failed to find the MAC of interface "%s" (%s); '
                            "it no longer exists",
                            event.existing.name,
                            event.existing.id_,
                        )
                        continue  # this is fine and sometimes even expected

                    mac = (
                        MacAddress(bytes(get_mac_response.mac.octets))
                        if get_mac_response.mac
                        else None
                    )
                except (FcTransportStatus, ZxStatus) as status:
                    raise HoneydewNetstackError(
                        f"Interfaces.GetMac() error {status}"
                    ) from status

                properties.append(
                    InterfaceProperties.from_fidl(event.existing, mac)
                )
            elif event.idle:
                # No more information readily available.
                break
            else:
                raise HoneydewNetstackError(
                    "Received invalid Watcher event from netstack. "
                    f"Expected existing or idle events, got {event}"
                )

        return properties

    async def ping(
        self,
        dest_ip: str,
        *,
        count: int = 3,
        interval: timedelta = timedelta(seconds=1),
        timeout: timedelta = timedelta(seconds=1),
        size: int = 25,
        additional_ping_params: str | None = None,
    ) -> PingResult:
        """Send ICMP echo requests to a destination.

        Args:
            dest_ip: Destination IP address or hostname.
            count: Number of packets to send.
            interval: Interval between packets.
            timeout: Timeout for each packet.
            size: Packet size in bytes.
            additional_ping_params: Additional parameters to pass to the ping command.

        Returns:
            Result of the ping operation.

        Raises:
            HoneydewNetstackError: Error executing ping.
        """
        cmd_str = (
            f"ping -c {count} -i {int(interval.total_seconds() * 1000)} "
            f"-t {int(timeout.total_seconds() * 1000)} -s {size}"
        )
        if additional_ping_params:
            cmd_str += f" {additional_ping_params}"
        cmd_str += f" {dest_ip}"

        _LOGGER.debug("Executing netstack ping: %s", cmd_str)

        stdout_str = ""

        try:
            stdout_str = self.ffx.run_ssh_cmd(cmd_str, capture_output=True)
        except ffx_errors.FfxCommandError as e:
            cause = e.__cause__
            while cause is not None and not isinstance(
                cause, subprocess.CalledProcessError
            ):
                cause = cause.__cause__

            if isinstance(cause, subprocess.CalledProcessError):
                stdout_str = cause.stdout or ""
                stderr_str = cause.stderr or ""
                raise HoneydewNetstackError(
                    f"Ping command '{cmd_str}' failed (exit status {cause.returncode}).\n"
                    f"Stdout: {stdout_str}\nStderr: {stderr_str}"
                ) from e
            else:
                raise HoneydewNetstackError(
                    f"Failed to execute ping command '{cmd_str}': {e}"
                ) from e
        except Exception as e:
            raise HoneydewNetstackError(
                f"Unexpected error executing ping '{cmd_str}': {e}"
            ) from e
        stats_match = re.search(
            r"(\d+) packets transmitted, (\d+) received, (\d+)% packet loss",
            stdout_str,
        )
        rtt_match = re.search(
            r"RTT Min/Max/Avg = \[ ([0-9.]+) / ([0-9.]+) / ([0-9.]+) \] ms",
            stdout_str,
        )

        if stats_match:
            transmitted = int(stats_match.group(1))
            received = int(stats_match.group(2))
        else:
            # Fallback if summary line is missing
            received = len(
                re.findall(
                    r"\d+ bytes from .* icmp_seq=\d+ rtt=.* ms", stdout_str
                )
            )

            # Stats line considers all failed packets as "transmitted"
            failures = len(
                re.findall(
                    r"ping: Could not send packet|ping: Timeout after",
                    stdout_str,
                )
            )

            if received > 0 or failures > 0:
                transmitted = received + failures
            else:
                raise HoneydewNetstackError(
                    f"Failed to parse ping statistics from output:\n{stdout_str}"
                )

        return PingResult(
            raw_output=stdout_str,
            requested=count,
            transmitted=transmitted,
            received=received,
            time_ms=None,
            rtt_min_ms=float(rtt_match.group(1)) if rtt_match else None,
            rtt_avg_ms=float(rtt_match.group(3)) if rtt_match else None,
            rtt_max_ms=float(rtt_match.group(2)) if rtt_match else None,
            rtt_mdev_ms=None,
        )

    async def wait_for_ipv4_addr(
        self,
        interface_id: int,
        timeout: timedelta = timedelta(seconds=30),
    ) -> IPv4Address:
        """Waits for an interface with the specified ID to have an IPv4 address.

        Args:
            interface_id: Interface ID to wait for.
            timeout: Max time to wait.

        Returns:
            The first assigned IPv4Address on the interface.

        Raises:
            HoneydewNetstackError: If timeout occurs before an address is assigned.
        """
        end_time = time.time() + timeout.total_seconds()
        while time.time() < end_time:
            interfaces = await self.list_interfaces()
            for iface in interfaces:
                if iface.id_ == interface_id and iface.ipv4_addresses:
                    return iface.ipv4_addresses[0]
            await asyncio.sleep(1)
        raise HoneydewNetstackError(
            f"Timed out after {timeout.total_seconds():.0f} seconds waiting for an IPv4 address"
            f" on interface ID {interface_id}"
        )

    async def wait_for_ipv6_addr(
        self,
        interface_id: int,
        timeout: timedelta = timedelta(seconds=30),
    ) -> IPv6Address:
        """Waits for an interface with the specified ID to have an IPv6 address.

        Args:
            interface_id: Interface ID to wait for.
            timeout: Max time to wait.

        Returns:
            The first assigned IPv6Address on the interface.

        Raises:
            HoneydewNetstackError: If timeout occurs before an address is assigned.
        """
        end_time = time.time() + timeout.total_seconds()
        while time.time() < end_time:
            interfaces = await self.list_interfaces()
            for iface in interfaces:
                if iface.id_ == interface_id and iface.ipv6_addresses:
                    return iface.ipv6_addresses[0]
            await asyncio.sleep(1)
        raise HoneydewNetstackError(
            f"Timed out after {timeout.total_seconds():.0f} seconds waiting for an IPv6 address"
            f" on interface ID {interface_id}"
        )

    async def wait_for_interface(
        self,
        port_class: PortClass,
        timeout: timedelta = timedelta(seconds=30),
    ) -> InterfaceProperties:
        """Waits for an interface with the specified port class to become available.

        Args:
            port_class: Port class to wait for.
            timeout: Max time to wait.

        Returns:
            Interface properties of the matching interface.

        Raises:
            HoneydewNetstackError: If timeout occurs before interface is found.
        """
        end_time = time.time() + timeout.total_seconds()
        while time.time() < end_time:
            interfaces = await self.list_interfaces()
            for interface in interfaces:
                if interface.port_class is port_class:
                    return interface
            await asyncio.sleep(1)
        raise HoneydewNetstackError(
            f"Timed out after {timeout.total_seconds():.0f} seconds waiting for a {port_class.name} interface"
        )
