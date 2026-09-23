# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import logging

import fidl_fuchsia_bluetooth as bluetooth
import fidl_fuchsia_bluetooth_sys as bluetooth_sys
import fuchsia_async_extension
from fuchsia_controller_py import Channel, ZxStatus
from mobly import test_runner
from mobly_controller import fuchsia_device

PEER_MATCH_TIMEOUT_SECONDS: int = 120


class MultipleFuchsiaDevicesNotFound(Exception):
    """Raised when there are less than two Fuchsia devices available."""


class BluetoothDevice(object):
    """A wrapper around a FuchsiaDevice that adds support for bluetooth."""

    def __init__(self, device: fuchsia_device.FuchsiaDevice):
        self.device = device
        self.discoverable_token: Channel | None = None
        self.discovery_token: Channel | None = None
        self.peer_update_task: asyncio.Task[None] | None = None
        self.peer_update_queue: asyncio.Queue[bluetooth_sys.Peer] | None = None

    def connect_proxies(self) -> None:
        if self.device.ctx is None:
            raise ValueError(f"Device: {self.device.target} has no context")
        self.access_proxy = bluetooth_sys.AccessClient(
            self.device.ctx.connect_device_proxy(
                "core/bluetooth-core", bluetooth_sys.AccessMarker
            )
        )
        self.host_watcher_proxy = bluetooth_sys.HostWatcherClient(
            self.device.ctx.connect_device_proxy(
                "core/bluetooth-core", bluetooth_sys.HostWatcherMarker
            )
        )

    async def start_listeners(self) -> None:
        queue: asyncio.Queue[bluetooth_sys.Peer] = asyncio.Queue()

        async def impl() -> None:
            while True:
                try:
                    results = await self.access_proxy.watch_peers()
                except ZxStatus as e:
                    if e.args[0] == ZxStatus.ZX_ERR_PEER_CLOSED:
                        break
                    raise e
                for peer in results.updated:
                    await queue.put(peer)

        self.peer_update_task = asyncio.get_running_loop().create_task(impl())
        self.peer_update_queue = queue

    def stop_listeners(self) -> None:
        if self.peer_update_task is None:
            raise ValueError(f"Device: {self.device.target} has no task")
        self.peer_update_task.cancel()
        self.peer_update_task = None
        self.peer_update_queue = None

    async def get_next_peer_update(self) -> bluetooth_sys.Peer:
        if self.peer_update_queue is None:
            raise ValueError(f"Device: {self.device.target} has no queue")
        res = await self.peer_update_queue.get()
        self.peer_update_queue.task_done()
        return res

    async def set_discoverable(self, enabled: bool) -> None:
        if enabled:
            client, server = self.device.channel_create()
            await self.access_proxy.make_discoverable(token=server.take())
            self.discoverable_token = client
        else:
            self.discoverable_token = None

    async def start_discovery(self) -> None:
        client, server = self.device.channel_create()
        await self.access_proxy.start_discovery(token=server.take())
        self.discovery_token = client

    def stop_discovery(self) -> None:
        self.discover_token = None

    async def get_adapter_address(self) -> bluetooth.Address:
        while True:
            hosts_response = await self.host_watcher_proxy.watch()
            hosts = hosts_response.hosts
            if hosts:
                for host in hosts:
                    if host.addresses:
                        res = host.addresses[0]
                        return res
                raise RuntimeError(
                    f"No addresses found in response: {hosts_response}"
                )
                break

    def cancel_peer_update_task(self) -> None:
        self.peer_update_task = None


class MultiDeviceTest(fuchsia_async_extension.AsyncBaseTestClass):
    def _setup_device(
        self, device: fuchsia_device.FuchsiaDevice
    ) -> BluetoothDevice:
        device.set_ctx(self)
        res = BluetoothDevice(device)
        res.connect_proxies()
        return res

    async def setup_class(self) -> None:
        self.fuchsia_devices = [
            self._setup_device(x)
            for x in await self.register_controller(fuchsia_device)
        ]
        if len(self.fuchsia_devices) < 2:
            raise MultipleFuchsiaDevicesNotFound(
                "Two fuchsia devices are required to run this test."
            )
        self.initiator = self.fuchsia_devices[0]
        self.receiver = self.fuchsia_devices[1]

    async def _wait_for_matching_peer(
        self, receiver_address: bluetooth.Address
    ) -> None:
        while True:
            peer = await self.initiator.get_next_peer_update()
            logging.debug(f"Received peer update: {peer}")
            if (
                peer.address is not None
                and peer.address.bytes_ == receiver_address.bytes_
                and peer.address.type_ == receiver_address.type_
            ):
                break

    async def test_discovery(self) -> None:
        for device in self.fuchsia_devices:
            await device.start_listeners()
            await device.start_discovery()
            await device.set_discoverable(True)
        receiver_address = await self.receiver.get_adapter_address()
        logging.info(f"Got receiver address: {receiver_address}")
        async with asyncio.timeout(PEER_MATCH_TIMEOUT_SECONDS):
            await self._wait_for_matching_peer(receiver_address)

    async def teardown_test(self) -> None:
        for device in self.fuchsia_devices:
            device.stop_listeners()
            await device.set_discoverable(False)
            device.stop_discovery()
            device.cancel_peer_update_task()


if __name__ == "__main__":
    test_runner.main()
