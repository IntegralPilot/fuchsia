# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Bluetooth LE affordance implementation using Fuchsia Controller."""

import asyncio
import logging
import uuid
from collections.abc import Sequence

import fidl_fuchsia_bluetooth as f_bt
import fidl_fuchsia_bluetooth_gatt2 as f_gatt_controller
import fidl_fuchsia_bluetooth_le as f_ble_controller
import fuchsia_controller_py as fc
from honeydew import affordances_capable
from honeydew.affordances.connectivity.bluetooth.bluetooth_common import (
    bluetooth_common_using_fc,
)
from honeydew.affordances.connectivity.bluetooth.le import le
from honeydew.affordances.connectivity.bluetooth.utils import (
    errors as bt_errors,
)
from honeydew.affordances.connectivity.bluetooth.utils import types as bt_types
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing import custom_types

_LOGGER: logging.Logger = logging.getLogger(__name__)


_FC_PROXIES: dict[str, custom_types.FidlEndpoint] = {
    "BluetoothLEPeripheral": custom_types.FidlEndpoint(
        "core/bluetooth-core", "fuchsia.bluetooth.le.Peripheral"
    ),
    "BluetoothLECentral": custom_types.FidlEndpoint(
        "core/bluetooth-core", "fuchsia.bluetooth.le.Central"
    ),
    "BluetoothLEScanWatcher": custom_types.FidlEndpoint(
        "core/bluetooth-core", "fuchsia.bluetooth.le.ScanResultWatcher"
    ),
    "BluetoothGattServer": custom_types.FidlEndpoint(
        "core/bluetooth-core", "fuchsia.bluetooth.gatt2.Server"
    ),
}

ASYNC_OP_TIMEOUT: int = 10


class AdvertisedPeripheralImpl(f_ble_controller.AdvertisedPeripheralServer):
    def on_connected(
        self, request: f_ble_controller.AdvertisedPeripheralOnConnectedRequest
    ) -> None:
        assert request.peer.id_ is not None
        _LOGGER.info(
            "Advertised Peripheral Connected with peer: %s",
            request.peer.id_.value,
        )
        self._peripheral_connection = request.connection


class LEUsingFc(le.LE, bluetooth_common_using_fc.BluetoothCommonUsingFc):
    """Async BluetoothLE Common affordance implementation using Fuchsia Controller.

    Args:
        device_name: Device name returned by `ffx target list`.
        fuchsia_controller: FC transport.
    """

    def __init__(
        self,
        device_name: str,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
    ) -> None:
        super().__init__(
            device_name=device_name,
            fuchsia_controller=fuchsia_controller,
            reboot_affordance=reboot_affordance,
        )
        self.service_info: dict[int, f_gatt_controller.ServiceInfo] = dict()
        self.known_le_devices: dict[int, f_ble_controller.Peer] = dict()
        self._peripheral_advertisement_server: asyncio.Task[None] | None = None
        self._device_name: str = device_name
        self._name: str = device_name
        self._connection_client: f_ble_controller.ConnectionClient | None = None
        self._gatt_client: f_gatt_controller.ClientClient | None = None
        self._remote_service_client: f_gatt_controller.RemoteServiceClient | None = (
            None
        )
        self._peripheral_connection: fc.Channel | None = None
        self._fc_transport: fc_transport.FuchsiaController = fuchsia_controller
        self._reboot_affordance: affordances_capable.RebootCapableDevice = (
            reboot_affordance
        )
        self._peripheral_controller_proxy: (
            f_ble_controller.PeripheralClient | None
        ) = None
        self._central_controller_proxy: (
            f_ble_controller.CentralClient | None
        ) = None

        self._gatt_server_proxy: f_gatt_controller.ServerClient | None = None
        self._le_session_initialized = False
        self._reboot_affordance.register_for_on_device_boot(fn=self.init_le_sys)
        self.verify_supported()
        self.init_le_sys()

    def verify_supported(self) -> None:
        """Check if Bluetooth le is supported on the DUT.
        Raises:
            NotSupportedError: Bluetooth Le affordance is not supported by Fuchsia device.
        """
        # TODO(http://b/409623831): Implement the method logic

    async def reset_state(self) -> None:
        """Reset the internal state tracking variables to correspond to an inactive BLE State."""
        self._peripheral_controller_proxy = None
        self._central_controller_proxy = None
        self._gatt_server_proxy = None
        self._remote_service_client = None
        if self._peripheral_advertisement_server is not None:
            _LOGGER.debug(
                "Cancelling Peripheral Advertisement Server and setting to None"
            )
            self._peripheral_advertisement_server.cancel()
            self._peripheral_advertisement_server = None
        self._peripheral_connection = None
        self._le_session_initialized = False
        # Explicitly call the common reset to bypass the interface's abstract method shadowing
        await bluetooth_common_using_fc.BluetoothCommonUsingFc.reset_state(self)

    def init_le_sys(self) -> None:
        """Initializes BLE stack.

        Note: This method is called automatically:
            1. During this class initialization
            2. After the device reboot

        Raises:
            BluetoothStateError: On failure.
        """

        if self._le_session_initialized:
            raise bt_errors.BluetoothStateError(
                f"Bluetooth session is already initialized on {self._device_name}. Can be "
                "initialized only once."
            )

        assert self._peripheral_controller_proxy is None
        self._peripheral_controller_proxy = f_ble_controller.PeripheralClient(
            self._fc_transport.connect_device_proxy(
                _FC_PROXIES["BluetoothLEPeripheral"]
            )
        )

        assert self._central_controller_proxy is None
        self._central_controller_proxy = f_ble_controller.CentralClient(
            self._fc_transport.connect_device_proxy(
                _FC_PROXIES["BluetoothLECentral"]
            )
        )
        assert self._gatt_server_proxy is None
        self._gatt_server_proxy = f_gatt_controller.ServerClient(
            self._fc_transport.connect_device_proxy(
                _FC_PROXIES["BluetoothGattServer"]
            )
        )
        self._le_session_initialized = True
        self.known_le_devices.clear()
        self.service_info.clear()
        self._uuid = f_bt.Uuid(value=self._generate_random_bluetooth_uuid())

    async def stop_advertise(self) -> None:
        """Stop advertising the peripheral."""
        self._peripheral_advertisement_server = None

    async def scan(self) -> list[f_ble_controller.Peer]:
        """Perform an LE scan on central device.

        Returns:
            A list of all known LE remote devices.
        """
        (
            central_client,
            central_server,
        ) = self._fc_transport.channel_create()
        watcher = f_ble_controller.ScanResultWatcherClient(central_client)
        filter_options = f_ble_controller.Filter()
        scan_options = f_ble_controller.ScanOptions(filters=[filter_options])
        assert self._central_controller_proxy is not None
        try:
            async with asyncio.timeout(ASYNC_OP_TIMEOUT):
                async with asyncio.TaskGroup() as tg:
                    task1 = tg.create_task(
                        self._central_controller_proxy.scan(
                            options=scan_options,
                            result_watcher=central_server.take(),
                        )
                    )
                    res = await watcher.watch()
                    central_client.close()
                    await task1
        except TimeoutError:
            _LOGGER.info(
                "No updates on % from watcher.watch(), returning cached peers.",
                self._device_name,
            )
            return list(self.known_le_devices.values())
        for peer in res.updated:
            assert peer.id_ is not None
            self.known_le_devices[peer.id_.value] = peer
        return list(self.known_le_devices.values())

    async def connect(self, identifier: f_bt.PeerId) -> None:
        """Initiate connection from the central device to peripheral.

        Args:
            identifier: the identifier of target remote device.

        Raises:
            BluetoothError: If the peripheral is not initialized.
        """
        (conn_client, conn_server) = self._fc_transport.channel_create()
        self._connection_client = f_ble_controller.ConnectionClient(
            conn_client.take()
        )
        connection_options = f_ble_controller.ConnectionOptions(
            bondable_mode=True
        )
        try:
            assert self._central_controller_proxy is not None
            self._central_controller_proxy.connect(
                id_=identifier,
                options=connection_options,
                handle=conn_server.take(),
            )
            # TODO: b/342432248 - Reduce sleep values to minimum stables values
            await asyncio.sleep(5)
        except Exception as e:  # pylint: disable=broad-except
            raise bt_errors.BluetoothError(
                f"Failed to complete BLE connect FIDL on {self._device_name}."
            ) from e

    async def advertise(
        self, appearance: bt_types.BluetoothLEAppearance, name: str
    ) -> None:
        """Advertise the peripheral.

        Args:
            appearance: Peripheral device appearance.
            name: Peripheral device name.
        """
        try:
            await self._advertise(appearance, name)
        except Exception as e:
            raise bt_errors.BluetoothError(
                f"Failed to complete BLE advertise FIDL call on {self._device_name}."
            ) from e

    async def _advertise(
        self, appearance: bt_types.BluetoothLEAppearance, name: str
    ) -> None:
        """Async function to advertise the peripheral.

        Args:
            appearance: Peripheral device appearance.
            name: Peripheral device name.
        """
        connection_options = f_ble_controller.ConnectionOptions(
            bondable_mode=True
        )
        advertising_data = f_ble_controller.AdvertisingData(
            name=name, appearance=appearance, service_uuids=[self._uuid]
        )
        params = f_ble_controller.AdvertisingParameters(
            data=advertising_data, connection_options=connection_options
        )
        (client, server) = self._fc_transport.channel_create()
        advertised_server = AdvertisedPeripheralImpl(server)
        self._peripheral_advertisement_server = (
            asyncio.get_running_loop().create_task(advertised_server.serve())
        )
        assert self._peripheral_controller_proxy is not None
        await self._peripheral_controller_proxy.advertise(
            parameters=params, advertised_peripheral=client.take()
        )

    async def wait_for_connection(self) -> None:
        """Function to run Advertised Peripheral server calls and wait for connection."""
        if self._peripheral_advertisement_server is None:
            raise bt_errors.BluetoothError(
                "No Peripheral Advertisement server active on "
                f"device: {self._device_name}"
            )
        try:
            async with asyncio.timeout(ASYNC_OP_TIMEOUT):
                await self._peripheral_advertisement_server
        except TimeoutError as e:
            raise bt_errors.BluetoothError(
                f"Failed to complete Peripheral connection calls on {self._device_name}."
            ) from e

    def _generate_random_bluetooth_uuid(self) -> list[int]:
        """Generates a random Bluetooth UUID in its 128-bit canonical form,
        then converts the bytes into little-endian order. Finally, convert
        to a list in big-endian order.

        Returns:
            list: A list of 16 integers representing the UUID in big-endian byte order.
        """

        random_uuid = uuid.uuid4()
        uuid_bytes = random_uuid.bytes_le
        # Convert to a list of integers in big-endian order
        uuid_list = list(uuid_bytes)
        return uuid_list

    async def publish_service(self) -> f_bt.Uuid:
        """Publish the Gatt service from the peripheral.

        Returns:
            The UUID of the service.

        Raises:
            NotImplementedError
        """
        raise NotImplementedError

    async def request_gatt_client(self) -> None:
        """Request the Gatt Client.

        Raises:
            BluetoothError: If the peripheral fails to request the Gatt client.
        """
        try:
            assert (
                self._connection_client is not None
            )  # the central connection handle should request Gatt client
            (client, server) = self._fc_transport.channel_create()
            gatt_client = f_gatt_controller.ClientClient(client.take())
            self._connection_client.request_gatt_client(
                client=server.take()
            )  # bind server end of gatt2 client
            self._gatt_client = gatt_client
        except Exception as e:
            raise bt_errors.BluetoothError(
                f"Failed to complete Request Gatt client FIDL call on {self._device_name}."
            ) from e

    async def list_gatt_services(
        self,
    ) -> list[f_gatt_controller.ServiceInfo]:
        """List the Gatt Services found on the connected peripheral.

        Returns:
            The list of Gatt Services of the connected peripheral.

        Raises:
            BluetoothError: If the device fails to complete the FIDL request.
        """
        try:
            assert (
                self._gatt_client is not None
            )  # the gatt client should not be none
            async with asyncio.timeout(ASYNC_OP_TIMEOUT):
                res = await self._gatt_client.watch_services(uuids=[])
        except TimeoutError:
            _LOGGER.info(
                "No updates on {self._device_name} from watch_services(), returning cache."
            )
            return list(self.service_info.values())
        for service in res.updated:
            assert service.handle is not None
            self.service_info[service.handle.value] = service
        for handle in res.removed:
            del self.service_info[handle.value]
        return list(self.service_info.values())

    async def connect_to_service(
        self, handle: f_gatt_controller.ServiceHandle
    ) -> None:
        """Connect to an available GATT service on the peripheral device.

        Args:
            handle: The handle of the service.

        Raises:
            BluetoothError: If the central device fails to connect to Gatt service.
        """
        try:
            assert self._gatt_client is not None
            (client, server) = self._fc_transport.channel_create()
            self._gatt_client.connect_to_service(
                handle=handle, service=server.take()
            )
            remote_client = f_gatt_controller.RemoteServiceClient(client.take())
            self._remote_service_client = remote_client
        except Exception as e:
            raise bt_errors.BluetoothError(
                f"Failed to complete connect_to_service FIDL call on {self._device_name}."
            ) from e

    async def discover_characteristics(
        self,
    ) -> Sequence[f_gatt_controller.Characteristic]:
        """Discover characteristics of a connected Gatt Service.

        Returns:
            The available characteristics of a connected Gatt Service.
        """
        try:
            assert self._remote_service_client is not None
            async with asyncio.timeout(ASYNC_OP_TIMEOUT):
                return (
                    await self._remote_service_client.discover_characteristics()
                ).characteristics
        except Exception as e:
            raise bt_errors.BluetoothError(
                f"Failed to complete discover_characteristics FIDL call on {self._device_name}."
            ) from e

    async def read_characteristic(
        self, handle: f_gatt_controller.Handle
    ) -> f_gatt_controller.RemoteServiceReadCharacteristicResponse:
        """Read characteristic of the Gatt service.

        Args:
            handle: The handle of the service.

        Returns:
            A characteristic of the Gatt service and its properties

        Raises:
            BluetoothError: If the peripheral fails to read the characteristic.
        """
        try:
            assert (
                self._remote_service_client is not None
            )  # we must have a connected client to make a remote service
            read_options = f_gatt_controller.ReadOptions(
                short_read=f_gatt_controller.ShortReadOptions()
            )
            async with asyncio.timeout(ASYNC_OP_TIMEOUT):
                return (
                    await self._remote_service_client.read_characteristic(
                        handle=handle, options=read_options
                    )
                ).unwrap()
        except Exception as e:
            raise bt_errors.BluetoothError(
                f"Failed to complete read_characteristics FIDL call on {self._device_name}."
            ) from e
