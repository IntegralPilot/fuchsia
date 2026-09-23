# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.connectivity.netstack."""

import asyncio
import subprocess
import types
import unittest
from datetime import timedelta
from ipaddress import IPv4Address, IPv6Address
from typing import TypeVar
from unittest import mock

import fidl_fuchsia_net as f_net
import fidl_fuchsia_net_interfaces as f_net_interfaces
import fidl_fuchsia_net_root as f_net_root
import fuchsia_controller_py as fc
from fuchsia_controller_py import Channel, ZxStatus
from honeydew import affordances_capable
from honeydew import errors as honeydew_errors
from honeydew.affordances.connectivity import netstack
from honeydew.affordances.connectivity.netstack.errors import (
    HoneydewNetstackError,
)
from honeydew.affordances.connectivity.netstack.types import (
    InterfaceProperties,
    PortClass,
)
from honeydew.errors import NotSupportedError
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing.custom_types import MacAddress

_TEST_MAC: MacAddress = MacAddress("12:34:56:78:90:ab")
_TEST_INTERFACE_ID: int = 1

_T = TypeVar("_T")


async def _async_response(response: _T) -> _T:
    return response


# pylint: disable=protected-access
class NetstackFCTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for honeydew.affordances.connectivity.netstack."""

    def setUp(self) -> None:
        super().setUp()
        self.reboot_affordance_obj = mock.MagicMock(
            spec=affordances_capable.RebootCapableDevice,
            autospec=True,
        )
        self.fc_transport_obj = mock.MagicMock(
            spec=fc_transport.FuchsiaController,
            autospec=True,
        )
        self.fc_transport_obj.ctx = fc.Context()

        def channel_create(
            self: fc_transport.FuchsiaController,
        ) -> tuple[fc.Channel, fc.Channel]:
            return self.ctx.channel_create()

        self.ffx_transport_obj = mock.MagicMock(
            spec=ffx_transport.FFX,
            autospec=True,
        )

        self.fc_transport_obj.channel_create = types.MethodType(
            channel_create, self.fc_transport_obj
        )

        self.ffx_transport_obj.run.return_value = "".join(
            netstack._REQUIRED_CAPABILITIES
        )

        self.netstack_obj = netstack.Netstack(
            device_name="fuchsia-emulator",
            ffx=self.ffx_transport_obj,
            fuchsia_controller=self.fc_transport_obj,
            reboot_affordance=self.reboot_affordance_obj,
        )

        self.watcher: asyncio.Task[None] | None = None

    def test_verify_supported(self) -> None:
        """Test if verify_supported works."""
        self.ffx_transport_obj.run.return_value = ""

        with self.assertRaises(NotSupportedError):
            netstack.Netstack(
                device_name="fuchsia-emulator",
                ffx=self.ffx_transport_obj,
                fuchsia_controller=self.fc_transport_obj,
                reboot_affordance=self.reboot_affordance_obj,
            )

    def test_init_register_for_on_device_boot(self) -> None:
        """Test if Netstack registers on_device_boot."""
        self.reboot_affordance_obj.register_for_on_device_boot.assert_called_once_with(
            self.netstack_obj._connect_proxy
        )

    def test_init_connect_proxy(self) -> None:
        """Test if Netstack connects to fuchsia.net.interface/State."""
        self.assertIsNotNone(self.netstack_obj._state_proxy)

    async def test_list_interfaces(self) -> None:
        """Test if list_interfaces works."""
        self.netstack_obj._state_proxy = mock.MagicMock(
            spec=f_net_interfaces.StateClient
        )

        def get_watcher(
            # pylint: disable-next=unused-argument
            options: f_net_interfaces.WatcherOptions,
            watcher: int,
        ) -> None:
            server = TestWatcherImpl(
                Channel(watcher),
                items=[
                    InterfaceProperties(
                        1,
                        "lo",
                        mac=_TEST_MAC,
                        ipv4_addresses=[IPv4Address("127.0.0.1")],
                        ipv6_addresses=[IPv6Address("fe80::1")],
                        port_class=PortClass.LOOPBACK,
                    ),
                    InterfaceProperties(
                        2,
                        "eth1",
                        mac=_TEST_MAC,
                        ipv4_addresses=[IPv4Address("192.168.42.1")],
                        ipv6_addresses=[],
                        port_class=PortClass.ETHERNET,
                    ),
                    InterfaceProperties(
                        3,
                        "wlan1",
                        mac=_TEST_MAC,
                        ipv4_addresses=[],
                        ipv6_addresses=[],
                        port_class=PortClass.WLAN_CLIENT,
                    ),
                ],
            )
            self.watcher = asyncio.create_task(server.serve())

        self.netstack_obj._state_proxy.get_watcher = mock.Mock(
            wraps=get_watcher,
        )

        self.netstack_obj._interfaces_proxy = mock.MagicMock(
            spec=f_net_root.InterfacesClient
        )

        mac_result = f_net_root.InterfacesGetMacResult(
            response=f_net_root.InterfacesGetMacResponse(
                mac=f_net.MacAddress(
                    octets=bytes(_TEST_MAC),
                ),
            )
        )

        self.netstack_obj._interfaces_proxy.get_mac.side_effect = [
            _async_response(mac_result),
            _async_response(mac_result),
            _async_response(mac_result),
        ]

        self.assertEqual(
            await self.netstack_obj.list_interfaces(),
            [
                InterfaceProperties(
                    1,
                    "lo",
                    mac=_TEST_MAC,
                    ipv4_addresses=[IPv4Address("127.0.0.1")],
                    ipv6_addresses=[IPv6Address("fe80::1")],
                    port_class=PortClass.LOOPBACK,
                ),
                InterfaceProperties(
                    2,
                    "eth1",
                    mac=_TEST_MAC,
                    ipv4_addresses=[IPv4Address("192.168.42.1")],
                    ipv6_addresses=[],
                    port_class=PortClass.ETHERNET,
                ),
                InterfaceProperties(
                    3,
                    "wlan1",
                    mac=_TEST_MAC,
                    ipv4_addresses=[],
                    ipv6_addresses=[],
                    port_class=PortClass.WLAN_CLIENT,
                ),
            ],
        )

    @mock.patch("asyncio.sleep", return_value=None)
    async def test_wait_for_ipv4_addr_success(
        self, mock_sleep: mock.Mock
    ) -> None:
        """Test if wait_for_ipv4_addr succeeds when the address appears."""
        self.netstack_obj._state_proxy = mock.MagicMock(
            spec=f_net_interfaces.StateClient
        )

        is_first_call = True

        def get_watcher(
            # pylint: disable-next=unused-argument
            options: f_net_interfaces.WatcherOptions,
            watcher: int,
        ) -> None:
            nonlocal is_first_call
            if is_first_call:
                is_first_call = False
                items = [
                    InterfaceProperties(
                        _TEST_INTERFACE_ID,
                        "wlan1",
                        mac=_TEST_MAC,
                        ipv4_addresses=[],
                        ipv6_addresses=[],
                        port_class=PortClass.WLAN_CLIENT,
                    )
                ]
            else:
                items = [
                    InterfaceProperties(
                        _TEST_INTERFACE_ID,
                        "wlan1",
                        mac=_TEST_MAC,
                        ipv4_addresses=[IPv4Address("192.168.1.1")],
                        ipv6_addresses=[],
                        port_class=PortClass.WLAN_CLIENT,
                    )
                ]

            server = TestWatcherImpl(Channel(watcher), items=items)
            self.watcher = asyncio.create_task(server.serve())

        self.netstack_obj._state_proxy.get_watcher = mock.Mock(
            wraps=get_watcher,
        )

        self.netstack_obj._interfaces_proxy = mock.MagicMock(
            spec=f_net_root.InterfacesClient
        )
        mac_result = f_net_root.InterfacesGetMacResult(
            response=f_net_root.InterfacesGetMacResponse(
                mac=f_net.MacAddress(octets=list(bytes(_TEST_MAC))),
            )
        )
        self.netstack_obj._interfaces_proxy.get_mac.side_effect = (
            lambda *args, **kwargs: _async_response(mac_result)
        )

        addr_v4 = await self.netstack_obj.wait_for_ipv4_addr(
            interface_id=_TEST_INTERFACE_ID
        )
        self.assertEqual(addr_v4, IPv4Address("192.168.1.1"))

    @mock.patch("asyncio.sleep", return_value=None)
    async def test_wait_for_ipv6_addr_success(
        self, mock_sleep: mock.Mock
    ) -> None:
        """Test if wait_for_ipv6_addr succeeds when the address appears."""
        self.netstack_obj._state_proxy = mock.MagicMock(
            spec=f_net_interfaces.StateClient
        )

        is_first_call = True

        def get_watcher(
            # pylint: disable-next=unused-argument
            options: f_net_interfaces.WatcherOptions,
            watcher: int,
        ) -> None:
            nonlocal is_first_call
            if is_first_call:
                is_first_call = False
                items = [
                    InterfaceProperties(
                        _TEST_INTERFACE_ID,
                        "wlan1",
                        mac=_TEST_MAC,
                        ipv4_addresses=[],
                        ipv6_addresses=[],
                        port_class=PortClass.WLAN_CLIENT,
                    )
                ]
            else:
                items = [
                    InterfaceProperties(
                        _TEST_INTERFACE_ID,
                        "wlan1",
                        mac=_TEST_MAC,
                        ipv4_addresses=[],
                        ipv6_addresses=[IPv6Address("fe80::2")],
                        port_class=PortClass.WLAN_CLIENT,
                    )
                ]

            server = TestWatcherImpl(Channel(watcher), items=items)
            self.watcher = asyncio.create_task(server.serve())

        self.netstack_obj._state_proxy.get_watcher = mock.Mock(
            wraps=get_watcher,
        )

        self.netstack_obj._interfaces_proxy = mock.MagicMock(
            spec=f_net_root.InterfacesClient
        )
        mac_result = f_net_root.InterfacesGetMacResult(
            response=f_net_root.InterfacesGetMacResponse(
                mac=f_net.MacAddress(octets=list(bytes(_TEST_MAC))),
            )
        )
        self.netstack_obj._interfaces_proxy.get_mac.side_effect = (
            lambda *args, **kwargs: _async_response(mac_result)
        )

        addr_v6 = await self.netstack_obj.wait_for_ipv6_addr(
            interface_id=_TEST_INTERFACE_ID
        )
        self.assertEqual(addr_v6, IPv6Address("fe80::2"))

    @mock.patch("asyncio.sleep", return_value=None)
    @mock.patch("time.time")
    async def test_wait_for_addr_timeout(
        self, mock_time: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        """Test if wait_for_ipv4_addr times out when address never appears."""
        mock_time.side_effect = [0.0, 3.0]

        self.netstack_obj._state_proxy = mock.MagicMock(
            spec=f_net_interfaces.StateClient
        )

        def get_watcher(
            # pylint: disable-next=unused-argument
            options: f_net_interfaces.WatcherOptions,
            watcher: int,
        ) -> None:
            items = [
                InterfaceProperties(
                    _TEST_INTERFACE_ID,
                    "wlan1",
                    mac=_TEST_MAC,
                    ipv4_addresses=[],
                    ipv6_addresses=[],
                    port_class=PortClass.WLAN_CLIENT,
                )
            ]
            server = TestWatcherImpl(Channel(watcher), items=items)
            self.watcher = asyncio.create_task(server.serve())

        self.netstack_obj._state_proxy.get_watcher = mock.Mock(
            wraps=get_watcher,
        )

        self.netstack_obj._interfaces_proxy = mock.MagicMock(
            spec=f_net_root.InterfacesClient
        )
        mac_result = f_net_root.InterfacesGetMacResult(
            response=f_net_root.InterfacesGetMacResponse(
                mac=f_net.MacAddress(octets=list(bytes(_TEST_MAC))),
            )
        )
        self.netstack_obj._interfaces_proxy.get_mac.side_effect = (
            lambda *args, **kwargs: _async_response(mac_result)
        )

        with self.assertRaises(HoneydewNetstackError) as context:
            await self.netstack_obj.wait_for_ipv4_addr(
                interface_id=_TEST_INTERFACE_ID, timeout=timedelta(seconds=2)
            )

        self.assertIn("Timed out", str(context.exception))

    @mock.patch("asyncio.sleep", return_value=None)
    async def test_wait_for_interface_success(
        self, mock_sleep: mock.Mock
    ) -> None:
        """Test if wait_for_interface succeeds when the interface appears."""
        self.netstack_obj._state_proxy = mock.MagicMock(
            spec=f_net_interfaces.StateClient
        )

        def get_watcher(
            # pylint: disable-next=unused-argument
            options: f_net_interfaces.WatcherOptions,
            watcher: int,
        ) -> None:
            items = [
                InterfaceProperties(
                    _TEST_INTERFACE_ID,
                    "wlan1",
                    mac=_TEST_MAC,
                    ipv4_addresses=[],
                    ipv6_addresses=[],
                    port_class=PortClass.WLAN_CLIENT,
                )
            ]
            server = TestWatcherImpl(Channel(watcher), items=items)
            self.watcher = asyncio.create_task(server.serve())

        self.netstack_obj._state_proxy.get_watcher = mock.Mock(
            wraps=get_watcher,
        )

        self.netstack_obj._interfaces_proxy = mock.MagicMock(
            spec=f_net_root.InterfacesClient
        )
        mac_result = f_net_root.InterfacesGetMacResult(
            response=f_net_root.InterfacesGetMacResponse(
                mac=f_net.MacAddress(octets=list(bytes(_TEST_MAC))),
            )
        )
        self.netstack_obj._interfaces_proxy.get_mac.side_effect = (
            lambda *args, **kwargs: _async_response(mac_result)
        )

        iface = await self.netstack_obj.wait_for_interface(
            port_class=PortClass.WLAN_CLIENT
        )
        self.assertEqual(iface.id_, _TEST_INTERFACE_ID)
        self.assertEqual(iface.name, "wlan1")
        self.assertEqual(iface.port_class, PortClass.WLAN_CLIENT)

    @mock.patch("asyncio.sleep", return_value=None)
    @mock.patch("time.time")
    async def test_wait_for_interface_timeout(
        self, mock_time: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        """Test if wait_for_interface times out when interface never appears."""
        mock_time.side_effect = [0.0, 3.0]

        self.netstack_obj._state_proxy = mock.MagicMock(
            spec=f_net_interfaces.StateClient
        )

        def get_watcher(
            # pylint: disable-next=unused-argument
            options: f_net_interfaces.WatcherOptions,
            watcher: int,
        ) -> None:
            items = [
                InterfaceProperties(
                    _TEST_INTERFACE_ID,
                    "eth1",
                    mac=_TEST_MAC,
                    ipv4_addresses=[],
                    ipv6_addresses=[],
                    port_class=PortClass.ETHERNET,
                )
            ]
            server = TestWatcherImpl(Channel(watcher), items=items)
            self.watcher = asyncio.create_task(server.serve())

        self.netstack_obj._state_proxy.get_watcher = mock.Mock(
            wraps=get_watcher,
        )

        self.netstack_obj._interfaces_proxy = mock.MagicMock(
            spec=f_net_root.InterfacesClient
        )
        mac_result = f_net_root.InterfacesGetMacResult(
            response=f_net_root.InterfacesGetMacResponse(
                mac=f_net.MacAddress(octets=list(bytes(_TEST_MAC))),
            )
        )
        self.netstack_obj._interfaces_proxy.get_mac.side_effect = (
            lambda *args, **kwargs: _async_response(mac_result)
        )

        with self.assertRaises(HoneydewNetstackError) as context:
            await self.netstack_obj.wait_for_interface(
                port_class=PortClass.WLAN_CLIENT, timeout=timedelta(seconds=2)
            )

        self.assertIn("Timed out", str(context.exception))

    async def test_ping_success(self) -> None:
        """Test successful ping execution and output parsing."""
        ping_output = (
            "Count: 3, Interval: 1000 ms, Timeout: 1000 ms, Message: This is an echo message!, Message size: 24 bytes, Source interface: (null), Destination: 8.8.8.8\n"
            "PING4 8.8.8.8 (8.8.8.8)\n"
            "33 bytes from 8.8.8.8 : icmp_seq=1 rtt=42.994 ms\n"
            "33 bytes from 8.8.8.8 : icmp_seq=2 rtt=22.378 ms\n"
            "33 bytes from 8.8.8.8 : icmp_seq=3 rtt=19.075 ms\n"
            "--- 8.8.8.8 ping statistics ---\n"
            "3 packets transmitted, 3 received, 0% packet loss, time 2008 ms\n"
            "RTT Min/Max/Avg = [ 19.075 / 42.994 / 28.149 ] ms\n"
        )
        self.ffx_transport_obj.run_ssh_cmd.return_value = ping_output

        res = await self.netstack_obj.ping("8.8.8.8", count=3)
        self.ffx_transport_obj.run_ssh_cmd.assert_called_once_with(
            "ping -c 3 -i 1000 -t 1000 -s 25 8.8.8.8", capture_output=True
        )
        self.assertEqual(res.raw_output, ping_output)
        self.assertEqual(res.requested, 3)
        self.assertTrue(res.all_pings_received)
        self.assertTrue(res.any_pings_received)
        self.assertEqual(res.transmitted, 3)
        self.assertEqual(res.received, 3)
        self.assertEqual(res.rtt_min_ms, 19.075)
        self.assertEqual(res.rtt_max_ms, 42.994)
        self.assertEqual(res.rtt_avg_ms, 28.149)

    async def test_ping_custom_interval_timeout(self) -> None:
        """Test ping with custom interval and timeout."""
        ping_output = (
            "Count: 2, Interval: 500 ms, Timeout: 2000 ms, Message: This is an echo message!, Message size: 24 bytes, Source interface: (null), Destination: 8.8.8.8\n"
            "PING4 8.8.8.8 (8.8.8.8)\n"
            "33 bytes from 8.8.8.8 : icmp_seq=1 rtt=42.994 ms\n"
            "33 bytes from 8.8.8.8 : icmp_seq=2 rtt=22.378 ms\n"
            "--- 8.8.8.8 ping statistics ---\n"
            "2 packets transmitted, 2 received, 0% packet loss, time 1008 ms\n"
            "RTT Min/Max/Avg = [ 22.378 / 42.994 / 32.686 ] ms\n"
        )
        self.ffx_transport_obj.run_ssh_cmd.return_value = ping_output

        res = await self.netstack_obj.ping(
            "8.8.8.8",
            count=2,
            interval=timedelta(milliseconds=500),
            timeout=timedelta(seconds=2),
        )
        self.ffx_transport_obj.run_ssh_cmd.assert_called_once_with(
            "ping -c 2 -i 500 -t 2000 -s 25 8.8.8.8", capture_output=True
        )
        self.assertEqual(res.raw_output, ping_output)
        self.assertEqual(res.requested, 2)
        self.assertTrue(res.all_pings_received)
        self.assertTrue(res.any_pings_received)
        self.assertEqual(res.transmitted, 2)
        self.assertEqual(res.received, 2)

    async def test_ping_partial_loss(self) -> None:
        """Test ping parsing when some packets are lost but exit status is 0."""
        ping_output = (
            "Count: 3, Interval: 1000 ms, Timeout: 1000 ms, Message: This is an echo message!, Message size: 24 bytes, Source interface: (null), Destination: 8.8.8.8\n"
            "PING4 8.8.8.8 (8.8.8.8)\n"
            "33 bytes from 8.8.8.8 : icmp_seq=1 rtt=42.994 ms\n"
            "ping: Timeout after 1000 ms\n"
            "33 bytes from 8.8.8.8 : icmp_seq=3 rtt=19.075 ms\n"
            "--- 8.8.8.8 ping statistics ---\n"
            "3 packets transmitted, 2 received, 33% packet loss, time 2008 ms\n"
            "RTT Min/Max/Avg = [ 19.075 / 42.994 / 31.034 ] ms\n"
        )
        self.ffx_transport_obj.run_ssh_cmd.return_value = ping_output

        res = await self.netstack_obj.ping("8.8.8.8", count=3)
        self.assertEqual(res.raw_output, ping_output)
        self.assertEqual(res.requested, 3)
        self.assertFalse(res.all_pings_received)
        self.assertTrue(res.any_pings_received)
        self.assertEqual(res.transmitted, 3)
        self.assertEqual(res.received, 2)
        self.assertEqual(res.rtt_min_ms, 19.075)
        self.assertEqual(res.rtt_max_ms, 42.994)
        self.assertEqual(res.rtt_avg_ms, 31.034)

    async def test_ping_fallback_parsing_success(self) -> None:
        """Test ping fallback parsing when summary line is missing but exit status is 0."""
        ping_output = (
            "Count: 3, Interval: 1000 ms, Timeout: 1000 ms, Message: This is an echo message!, Message size: 24 bytes, Source interface: (null), Destination: 8.8.8.8\n"
            "PING4 8.8.8.8 (8.8.8.8)\n"
            "33 bytes from 8.8.8.8 : icmp_seq=1 rtt=42.994 ms\n"
            "ping: Timeout after 1000 ms\n"
            "33 bytes from 8.8.8.8 : icmp_seq=3 rtt=19.075 ms\n"
            "ping: Could not send packet: Network unreachable\n"
        )
        self.ffx_transport_obj.run_ssh_cmd.return_value = ping_output

        res = await self.netstack_obj.ping("8.8.8.8", count=3)
        self.assertEqual(res.raw_output, ping_output)
        self.assertEqual(res.requested, 3)
        self.assertFalse(res.all_pings_received)
        self.assertTrue(res.any_pings_received)
        self.assertEqual(res.received, 2)
        self.assertEqual(res.transmitted, 4)

    async def test_ping_failure_cause_traversal(self) -> None:
        """Test ping failure extraction from CalledProcessError chain."""
        fail_output = (
            "Count: 3, Interval: 1000 ms, Timeout: 1000 ms, Message: This is an echo message!, Message size: 24 bytes, Source interface: (null), Destination: 8.8.8.8\n"
            "PING4 8.8.8.8 (8.8.8.8)\n"
            "ping: Timeout after 1000 ms\n"
            "ping: Timeout after 1000 ms\n"
            "ping: Timeout after 1000 ms\n"
            "--- 8.8.8.8 ping statistics ---\n"
            "3 packets transmitted, 0 received, 100% packet loss, time 2008 ms\n"
        )

        cpe = subprocess.CalledProcessError(
            returncode=1,
            cmd=["ffx", "target", "ssh", "ping", "..."],
            output=fail_output,
            stderr="some stderr",
        )
        hce = honeydew_errors.HostCmdError("Command failed...")
        hce.__cause__ = cpe
        fce = ffx_errors.FfxCommandError(hce)
        fce.__cause__ = hce

        self.ffx_transport_obj.run_ssh_cmd.side_effect = fce

        with self.assertRaises(HoneydewNetstackError) as context:
            await self.netstack_obj.ping("8.8.8.8", count=3)

        self.assertIn("exit status 1", str(context.exception))
        self.assertIn(fail_output, str(context.exception))
        self.assertIn("some stderr", str(context.exception))


class TestWatcherImpl(f_net_interfaces.WatcherServer):
    """Iterator for netstack events."""

    def __init__(
        self, server: Channel, items: list[InterfaceProperties]
    ) -> None:
        super().__init__(server)
        self._items = items
        self._done = False

    def watch(
        self,
    ) -> f_net_interfaces.WatcherWatchResponse:
        """Get next set of NetworkConfigs."""

        if len(self._items) == 0:
            if self._done:
                raise ZxStatus(ZxStatus.ZX_ERR_PEER_CLOSED)
            else:
                self._done = True
                # Indicate no more existing events will be sent.
                return f_net_interfaces.WatcherWatchResponse(
                    event=f_net_interfaces.Event(idle=f_net_interfaces.Empty())
                )

        return f_net_interfaces.WatcherWatchResponse(
            event=f_net_interfaces.Event(existing=self._items.pop(0).to_fidl())
        )


if __name__ == "__main__":
    unittest.main()
