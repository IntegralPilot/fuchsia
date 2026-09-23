# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Bluetooth Common affordance implementation using Fuchsia Controller."""

import asyncio
import logging

import fidl_fuchsia_bluetooth as f_bt
import fidl_fuchsia_bluetooth_sys as f_btsys_controller
import fuchsia_controller_py as fc
from honeydew import affordances_capable
from honeydew.affordances.connectivity.bluetooth.bluetooth_common import (
    bluetooth_common,
)
from honeydew.affordances.connectivity.bluetooth.utils import (
    errors as bluetooth_errors,
)
from honeydew.affordances.connectivity.bluetooth.utils import types as bt_types
from honeydew.affordances.connectivity.bluetooth.utils.fidl_servers import (
    bt_fidl_servers,
)
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing import custom_types

_LOGGER: logging.Logger = logging.getLogger(__name__)


class _FCBluetoothProxies:
    BT_SYS_ACCESS: custom_types.FidlEndpoint = custom_types.FidlEndpoint(
        "core/bluetooth-core", "fuchsia.bluetooth.sys.Access"
    )
    BT_SYS_HOST_WATCHER: custom_types.FidlEndpoint = custom_types.FidlEndpoint(
        "core/bluetooth-core", "fuchsia.bluetooth.sys.HostWatcher"
    )
    BT_SYS_PAIRING: custom_types.FidlEndpoint = custom_types.FidlEndpoint(
        "core/bluetooth-core", "fuchsia.bluetooth.sys.Pairing"
    )


_FC_DELEGATES: dict[str, int] = {
    "NONE": 1,
    "CONFIRMATION": 2,
    "KEYBOARD": 3,
}

BluetoothAcceptPairing = bt_types.BluetoothAcceptPairing
BluetoothConnectionType = bt_types.BluetoothConnectionType

# TODO: b/372516558 - Investigate to reduce async op seconds
ASYNC_OP_TIMEOUT: int = 30


class BluetoothCommonUsingFc(bluetooth_common.BluetoothCommon):
    """Bluetooth Common affordance implementation using Fuchsia Controller.

    Args:
        device_name: Device name returned by `ffx target list`.
        fuchsia_controller: Fuchsia Controller transport.
        reboot_affordance: Object that implements RebootCapableDevice.

    Raises:
        BluetoothStateError: On system initialization failure.
    """

    def __init__(
        self,
        device_name: str,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
    ) -> None:
        self._device_name = device_name
        self._fc_transport = fuchsia_controller
        self._reboot_affordance = reboot_affordance

        self.discoverable_token: fc.Channel | None = None
        self.discovery_token: fc.Channel | None = None
        self._pairing_delegate_server: asyncio.Task[None] | None = None
        self.known_devices: dict[
            custom_types.MacAddress, bt_types.BluetoothPeerInfo
        ]
        self._peer_update_task: asyncio.Task[None] | None = None
        self._peer_update_queue: (
            asyncio.Queue[f_btsys_controller.Peer] | None
        ) = None
        self._session_initialized: bool = False
        self._access_controller_proxy: (
            f_btsys_controller.AccessClient | None
        ) = None
        self._host_watcher_controller_proxy: (
            f_btsys_controller.HostWatcherClient | None
        ) = None
        self._pairing_controller_proxy: (
            f_btsys_controller.PairingClient | None
        ) = None

        # `sys_init` need to be called on every device bootup
        self._reboot_affordance.register_for_on_device_boot(fn=self.sys_init)

        self._async_op_count: int = 0
        self.sys_init()

    async def reset_state(self) -> None:
        """Resets internal state tracking variables to correspond to an inactive
        state; i.e. Bluetooth uninitialized and not started.
        """
        self._access_controller_proxy = None
        self._host_watcher_controller_proxy = None
        self._session_initialized = False
        self._pairing_controller_proxy = None
        if self._pairing_delegate_server is not None:
            _LOGGER.debug(
                "Cancelling Pairing Delegate Server and setting to None"
            )
            self._pairing_delegate_server.cancel()
            try:
                await self._pairing_delegate_server
            except asyncio.CancelledError:
                pass
            self._pairing_delegate_server = None

    def is_session_initialized(self) -> bool:
        """Checks if the BT session is initialized or not.

        Returns:
            True if the session is initialized, False otherwise.
        """
        return self._session_initialized

    def sys_init(self) -> None:
        """Initializes Bluetooth stack.

        Note: This method is called automatically:
            1. During this class initialization
            2. After the device reboot

        Raises:
            BluetoothStateError: On failure.
        """
        if self._session_initialized:
            raise bluetooth_errors.BluetoothStateError(
                f"Bluetooth session is already initialized on {self._device_name}. Can be "
                "initialized only once."
            )

        assert self._access_controller_proxy is None
        self._access_controller_proxy = f_btsys_controller.AccessClient(
            self._fc_transport.connect_device_proxy(
                _FCBluetoothProxies.BT_SYS_ACCESS
            )
        )
        assert self._host_watcher_controller_proxy is None
        self._host_watcher_controller_proxy = (
            f_btsys_controller.HostWatcherClient(
                self._fc_transport.connect_device_proxy(
                    _FCBluetoothProxies.BT_SYS_HOST_WATCHER
                )
            )
        )

        assert self._pairing_controller_proxy is None
        self._pairing_controller_proxy = f_btsys_controller.PairingClient(
            self._fc_transport.connect_device_proxy(
                _FCBluetoothProxies.BT_SYS_PAIRING
            )
        )
        self._session_initialized = True
        self.known_devices = {}

    def verify_supported(self) -> None:
        """Check if Bluetooth is supported on the DUT.
        Raises:
            NotSupportedError: Bluetooth affordance is not supported by Fuchsia device.
        """
        # TODO(http://b/409624089): Implement the method logic

    async def accept_pairing(
        self,
        input_mode: BluetoothAcceptPairing,
        output_mode: BluetoothAcceptPairing,
        timeout_sec: float | None = None,
    ) -> None:
        """Sets device to accept Bluetooth pairing.

        Args:
            input_mode: input mode of device
            output_mode: output mode of device
            timeout_sec: timeout duration in seconds

        Raises:
            BluetoothError: On failure.
        """
        self._async_op_count += 1
        try:
            async with asyncio.timeouts.timeout(timeout_sec):
                (tx, rx) = self._fc_transport.channel_create()
                server = bt_fidl_servers.PairingDelegateImpl(rx)
                self._pairing_delegate_server = asyncio.create_task(
                    server.serve()
                )
                assert self._pairing_controller_proxy is not None
                self._pairing_controller_proxy.set_pairing_delegate(
                    input_=_FC_DELEGATES[input_mode],
                    output=_FC_DELEGATES[output_mode],
                    delegate=tx.take(),
                )
                _LOGGER.debug("Pairing Delegate has been set.")
        except Exception as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete _accept_pairing FIDL call on {self._device_name}."
            ) from e

    async def connect_device(
        self,
        identifier: f_bt.PeerId,
        connection_type: BluetoothConnectionType = BluetoothConnectionType.CLASSIC,
        timeout_sec: float | None = None,
    ) -> None:
        """Connect to a peer device via Bluetooth.

        Args:
            identifier: the identifier of target remote device.
            connection_type: type of Bluetooth connection
            timeout_sec: timeout duration in seconds

        Raises:
            BluetoothError: On failure.
        """
        assert self._access_controller_proxy is not None
        self._async_op_count += 1
        try:
            await asyncio.wait_for(
                self._access_controller_proxy.connect(id_=identifier),
                timeout_sec,
            )
            # TODO(https://fxbug.dev/342432248): Reduce sleep values to minimum stables values
            self._async_op_count += 1
            await asyncio.sleep(10)
        except Exception as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete Bluetooth connect FIDL call on {self._device_name}."
            ) from e

    async def forget_device(
        self, identifier: f_bt.PeerId, timeout_sec: float | None = None
    ) -> None:
        """Forget and delete peer device via Bluetooth.

        Args:
            identifier: the identifier of target remote device.
            timeout_sec: timeout duration in seconds

        Raises:
            BluetoothError: On failure.
        """
        assert self._access_controller_proxy is not None
        self._async_op_count += 1
        try:
            await asyncio.wait_for(
                self._access_controller_proxy.forget(id_=identifier),
                timeout_sec,
            )
        except Exception as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete Bluetooth forget FIDL call on {self._device_name}."
            ) from e

    async def get_active_adapter_address(
        self, timeout_sec: float | None = None
    ) -> custom_types.MacAddress:
        """Retrieves the active adapter mac address

        Args:
            timeout_sec: timeout duration in seconds

        Sample result:
            {"result": "[address (public) 20:1F:3B:62:E9:D2]"}
        Returns:
            The mac address of the active adapter

        Raises:
            BluetoothError: If no addresses were found
        """
        self._async_op_count += 1
        try:
            return await asyncio.wait_for(
                self._get_active_address(), timeout_sec
            )
        except Exception as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete _get_active_address FIDL call on {self._device_name}."
            ) from e

    async def _get_active_address(self) -> custom_types.MacAddress:
        """Async private function to retrieve the active address.
                Sample result:
            {"result": "[address (public) 20:1F:3B:62:E9:D2]"}
        Returns:
            The mac address of the active adapter

        Raises:
            BluetoothStateError: If no addresses were found
        """
        assert self._host_watcher_controller_proxy is not None
        hosts_response = await self._host_watcher_controller_proxy.watch()
        hosts = hosts_response.hosts
        if hosts:
            for host in hosts:
                if host.addresses:
                    res = host.addresses[0]
                    return custom_types.MacAddress(bytes(res.bytes_))
        raise bluetooth_errors.BluetoothStateError(
            f"No Bluetooth addresses found on {self._device_name} in FIDL response: {hosts_response}"
        )

    async def get_connected_devices(self) -> list[str]:
        """Retrieves all connected remote devices.

        Returns:
            A list of all connected devices by identifier. If none,
            then returns empty list.

        """
        data = await self.get_known_remote_devices()
        connected_devices = []
        for peer in data.values():
            if peer.bonded:
                connected_devices.append(str(peer.id.value))
        return connected_devices

    async def get_known_remote_devices(
        self, timeout_sec: float | None = None
    ) -> dict[custom_types.MacAddress, bt_types.BluetoothPeerInfo]:
        """Retrieves all known remote devices received by device.

        Args:
            timeout_sec: timeout duration in seconds

        Returns:
            A dict of all known remote devices keyed by MacAddress.
        """
        self._async_op_count += 1
        try:
            assert self._access_controller_proxy is not None
            results = await asyncio.wait_for(
                self._access_controller_proxy.watch_peers(),
                timeout_sec,
            )
        except TimeoutError:
            _LOGGER.info(
                "No updates on {self._device_name} from watch_peers(), returning cached peers."
            )
            return self.known_devices
        for p in results.updated:
            if p.address and p.id_:
                mac_address = custom_types.MacAddress(bytes(p.address.bytes_))
                self.known_devices[mac_address] = bt_types.BluetoothPeerInfo(
                    id=p.id_,
                    address=list(p.address.bytes_),
                    connected=p.connected if p.connected is not None else False,
                    bonded=p.bonded if p.bonded is not None else False,
                    name=p.name if p.name else None,
                    appearance=p.appearance
                    if p.appearance is not None
                    else None,
                    rssi=p.rssi if p.rssi is not None else None,
                    services=list(p.services)
                    if p.services is not None
                    else None,
                    technology=int(p.technology)
                    if p.technology is not None
                    else None,
                    tx_power=p.tx_power if p.tx_power is not None else None,
                )
        return self.known_devices

    async def pair_device(
        self,
        identifier: f_bt.PeerId,
        connection_type: bt_types.BluetoothConnectionType = bt_types.BluetoothConnectionType.CLASSIC,
        timeout_sec: float | None = None,
    ) -> None:
        """Initiate pairing with peer device via Bluetooth.

        Args:
            identifier: the identifier of target remote device.
            connection_type: type of Bluetooth connection
            timeout_sec: timeout duration in seconds
        """
        self._async_op_count += 1
        options = f_btsys_controller.PairingOptions()
        try:
            assert self._access_controller_proxy is not None
            await asyncio.wait_for(
                self._access_controller_proxy.pair(
                    id_=identifier, options=options
                ),
                timeout_sec,
            )
            # TODO: b/342432248 - Reduce sleep values to minimum stables values
            self._async_op_count += 1
            await asyncio.sleep(10)
        except Exception as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete Bluetooth pair FIDL call on {self._device_name}."
            ) from e

    async def request_discovery(self, discovery: bool) -> None:
        """Start or stop Bluetooth Discovery on Bluetooth capable device.

        Args:
            discovery: True to start discovery, False to stop discovery.

        Raises:
            BluetoothError: If token is initialized.
        """
        if not discovery:
            self.discovery_token = None
            return
        if self.discovery_token is not None:
            raise bluetooth_errors.BluetoothError(
                "Cannot start discovery: Active discovery is "
                f"initialized on {self._device_name}"
            )
        client, server = self._fc_transport.channel_create()
        assert self._access_controller_proxy is not None
        self._async_op_count += 1
        try:
            await self._access_controller_proxy.start_discovery(
                token=server.take()
            )
        except Exception as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete Bluetooth start_discovery FIDL call on {self._device_name}."
            ) from e
        self.discovery_token = client

    async def set_discoverable(
        self,
        discoverable: bool,
    ) -> None:
        """Set or revoke Bluetooth discoverability.

        Args:
            discoverable: True to be discoverable by others, False to be not
                          discoverable by others.

        Raises:
            BluetoothError: If token is initialized.
        """
        if not discoverable:
            self.discoverable_token = None
            return
        if self.discoverable_token is not None:
            raise bluetooth_errors.BluetoothError(
                "Cannot turn on discoverability: discoverability is "
                f"initialized on {self._device_name}"
            )
        client, server = self._fc_transport.channel_create()
        assert self._access_controller_proxy is not None
        self._async_op_count += 1
        try:
            await self._access_controller_proxy.make_discoverable(
                token=server.take()
            )
        except Exception as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete Bluetooth make_discoverable FIDL call on {self._device_name}."
            ) from e
        self.discoverable_token = client

    async def run_pairing_delegate(
        self, timeout_sec: float | None = None
    ) -> None:
        """Function to run pairing delegate server calls.

        Args:
            timeout_sec: timeout duration in seconds

        Raises:
            BluetoothError: If token is initialized.
        """
        if self._pairing_delegate_server is None:
            raise bluetooth_errors.BluetoothError(
                "No pairing_delegate_server active on "
                f"device: {self._device_name}"
            )
        self._async_op_count += 1
        try:
            await asyncio.wait_for(self._pairing_delegate_server, timeout_sec)
        except Exception as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete Pairing Delegate Server calls on {self._device_name}."
            ) from e
