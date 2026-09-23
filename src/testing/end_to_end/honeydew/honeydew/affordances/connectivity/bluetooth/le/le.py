# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for Bluetooth LE Profile affordance."""

import abc
from collections.abc import Sequence

import fidl_fuchsia_bluetooth as f_bt
import fidl_fuchsia_bluetooth_gatt2 as f_gatt_controller
import fidl_fuchsia_bluetooth_le as f_ble_controller
from honeydew.affordances.connectivity.bluetooth.bluetooth_common import (
    bluetooth_common,
)
from honeydew.affordances.connectivity.bluetooth.utils import types as bt_types


class LE(bluetooth_common.BluetoothCommon):
    """Abstract base class for an async Bluetooth LE Profile affordance."""

    # TODO(b/352584355): Add functional tests for BLE affordance
    @abc.abstractmethod
    async def reset_state(self) -> None:
        """Reset the internal state tracking variables to correspond to an inactive BLE State."""

    @abc.abstractmethod
    async def advertise(
        self, appearance: bt_types.BluetoothLEAppearance, name: str
    ) -> None:
        """Advertise the peripheral.

        Args:
            appearance: Peripheral device appearance.
            name: Peripheral device name.

        Raises:
            BluetoothError: If the peripheral fails to advertise.
        """

    @abc.abstractmethod
    async def connect(self, identifier: f_bt.PeerId) -> None:
        """Initiate connection from the central device to peripheral.

        Args:
            identifier: The identifier of the peripheral.

        Raises:
            BluetoothError: If the peripheral fails to connect to central device.
        """

    @abc.abstractmethod
    async def connect_to_service(
        self, handle: f_gatt_controller.ServiceHandle
    ) -> None:
        """Connect to available Gatt services on the central device.

        Args:
            handle: The handle of the service.

        Raises:
            BluetoothError: If the central device fails to connect to Gatt service.
        """

    @abc.abstractmethod
    async def discover_characteristics(
        self,
    ) -> Sequence[f_gatt_controller.Characteristic]:
        """Discover characteristics of a connected Gatt Service.

        Returns:
            The available characteristics of a connected Gatt Service.
        """

    @abc.abstractmethod
    async def list_gatt_services(
        self,
    ) -> list[f_gatt_controller.ServiceInfo]:
        """List the Gatt Services found on the connected peripheral.

        Raises:
            BluetoothError: If the device fails to complete the FIDL request.
        """

    @abc.abstractmethod
    def init_le_sys(self) -> None:
        """Initializes ble stack.

        Note: This method is called automatically:
            1. During this class initialization
            2. After the device reboot

        Raises:
            errors.BluetoothStateError: On failure.
        """

    @abc.abstractmethod
    async def publish_service(self) -> f_bt.Uuid:
        """Publish the Gatt service from the peripheral.

        Returns:
            The UUID of the service.

        Raises:
            BluetoothError: If the peripheral fails to publish the Gatt service.
        """

    @abc.abstractmethod
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

    @abc.abstractmethod
    async def request_gatt_client(self) -> None:
        """Request the Gatt Client.

        Raises:
            BluetoothError: If the peripheral fails to request the Gatt client.
        """

    @abc.abstractmethod
    async def stop_advertise(self) -> None:
        """Stop advertising the peripheral."""

    @abc.abstractmethod
    async def wait_for_connection(self) -> None:
        """Wait for the peripheral to realize it has been connected to by a central.

        Raises:
            BluetoothError: If it fails to wait for connection.
        """

    @abc.abstractmethod
    async def scan(self) -> list[f_ble_controller.Peer]:
        """Perform an LE scan on central device.

        Returns:
            The scan result.

        Raises:
            BluetoothError: If the central device fails to complete the scan FIDL.
        """
