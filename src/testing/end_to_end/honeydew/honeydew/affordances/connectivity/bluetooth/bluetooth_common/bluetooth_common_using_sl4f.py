# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Bluetooth Common affordance implementation using SL4F."""

from enum import StrEnum

import fidl_fuchsia_bluetooth as f_bt
from honeydew import affordances_capable
from honeydew.affordances.connectivity.bluetooth.bluetooth_common import (
    bluetooth_common,
)
from honeydew.affordances.connectivity.bluetooth.utils import (
    errors as bluetooth_errors,
)
from honeydew.affordances.connectivity.bluetooth.utils import (
    types as bluetooth_types,
)
from honeydew.transports.sl4f import errors as sl4f_errors
from honeydew.transports.sl4f import sl4f as sl4f_transport
from honeydew.typing.custom_types import MacAddress


class Sl4fMethods(StrEnum):
    SET_DISCOVERABLE = "bt_sys_facade.BluetoothSetDiscoverable"
    INIT_SYS = "bt_sys_facade.BluetoothInitSys"
    REQUEST_DISCOVERY = "bt_sys_facade.BluetoothRequestDiscovery"
    GET_ACTIVE_ADDRESS = "bt_sys_facade.BluetoothGetActiveAdapterAddress"
    GET_KNOWN_REMOTE_DEVICES = "bt_sys_facade.BluetoothGetKnownRemoteDevices"
    ACCEPT_PAIRING = "bt_sys_facade.BluetoothAcceptPairing"
    PAIR_DEVICE = "bt_sys_facade.BluetoothPairDevice"
    CONNECT_DEVICE = "bt_sys_facade.BluetoothConnectDevice"
    FORGET_DEVICE = "bt_sys_facade.BluetoothForgetDevice"


class BluetoothCommonUsingSl4f(bluetooth_common.BluetoothCommon):
    """Bluetooth Common affordance implementation using SL4F.

    Args:
        device_name: Device name returned by `ffx target list`.
        sl4f: SL4F transport.
        reboot_affordance: Object that implements RebootCapableDevice.
    """

    def __init__(
        self,
        device_name: str,
        sl4f: sl4f_transport.SL4F,
        reboot_affordance: affordances_capable.RebootCapableDevice,
    ) -> None:
        self._name: str = device_name
        self._sl4f: sl4f_transport.SL4F = sl4f
        self._reboot_affordance: affordances_capable.RebootCapableDevice = (
            reboot_affordance
        )

        # `sys_init` need to be called on every device bootup
        self._reboot_affordance.register_for_on_device_boot(fn=self.sys_init)

        # Initialize the bluetooth stack
        self.sys_init()

    def sys_init(self) -> None:
        """Initializes bluetooth stack.

        Note: This method is called automatically:
            1. During this class initialization
            2. After the device reboot

        Raises:
            BluetoothError: On failure.
        """
        try:
            self._sl4f.run(method=Sl4fMethods.INIT_SYS)
        except sl4f_errors.Sl4fError as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete sys_init SL4F call on {self._name}."
            ) from e

    async def accept_pairing(
        self,
        input_mode: bluetooth_types.BluetoothAcceptPairing,
        output_mode: bluetooth_types.BluetoothAcceptPairing,
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
        try:
            self._sl4f.run(
                method=Sl4fMethods.ACCEPT_PAIRING,
                params={"input": input_mode, "output": output_mode},
            )
        except sl4f_errors.Sl4fError as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete accept_pairing SL4F call on {self._name}."
            ) from e

    async def connect_device(
        self,
        identifier: f_bt.PeerId,
        connection_type: bluetooth_types.BluetoothConnectionType,
        timeout_sec: float | None = None,
    ) -> None:
        """Connect device to target remote device via Bluetooth.

        Args:
            identifier: the identifier of target remote device.
            connection_type: type of bluetooth connection
            timeout_sec: timeout duration in seconds

        Raises:
            BluetoothError: On failure.
        """
        try:
            self._sl4f.run(
                method=Sl4fMethods.CONNECT_DEVICE,
                params={
                    "identifier": str(identifier.value),
                    "transport": connection_type.value,
                },
            )
        except sl4f_errors.Sl4fError as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete connect_device SL4F call on {self._name}."
            ) from e

    async def forget_device(
        self, identifier: f_bt.PeerId, timeout_sec: float | None = None
    ) -> None:
        """Forget device to target remote device via Bluetooth.

        Args:
            identifier: the identifier of target remote device.
            timeout_sec: timeout duration in seconds

        Raises:
            BluetoothError: On failure.
        """
        try:
            self._sl4f.run(
                method=Sl4fMethods.FORGET_DEVICE,
                params={"identifier": str(identifier.value)},
            )
        except sl4f_errors.Sl4fError as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete forget_device SL4F call on {self._name}."
            ) from e

    async def get_active_adapter_address(
        self, timeout_sec: float | None = None
    ) -> MacAddress:
        """Retrieves the active adapter mac address

        Args:
            timeout_sec: timeout duration in seconds

        Sample result:
            {"result": "[address (public) 20:1F:3B:62:E9:D2]"}
        Returns:
            The mac address of the active adapter

        Raises:
            BluetoothError: On failure.
            KeyError: On unexpected SL4F response
            AttributeError: On unexpected SL4F response
            IndexError: On unexpected SL4F response
        """
        try:
            address = self._sl4f.run(method=Sl4fMethods.GET_ACTIVE_ADDRESS)
            mac_address = address["result"].strip("[]").split(" ")
        except sl4f_errors.Sl4fError as e:
            raise bluetooth_errors.BluetoothError(
                "Failed to complete get_active_adapter_address SL4F call on "
                f"{self._name}."
            ) from e
        return MacAddress(mac_address[2])

    async def get_connected_devices(self) -> list[str]:
        """Retrieves all connected remote devices.

        Returns:
            A list of all connected devices by identifier. If none,
            then returns empty list.

        Raises:
            BluetoothError: On failure.
        """
        data = await self.get_known_remote_devices()
        connected_devices = []
        for peer in data.values():
            if peer.bonded:
                connected_devices.append(str(peer.id.value))
        return connected_devices

    async def get_known_remote_devices(
        self, timeout_sec: float | None = None
    ) -> dict[MacAddress, bluetooth_types.BluetoothPeerInfo]:
        """Retrieves all known remote devices received by device.
        Args:
            timeout_sec: timeout duration in seconds

        Returns:
            A dict of all known remote devices keyed by MacAddress.

        Raises:
            BluetoothError: On failure.
        """
        try:
            known_devices = self._sl4f.run(
                method=Sl4fMethods.GET_KNOWN_REMOTE_DEVICES
            )
            result = {}
            for value in known_devices.get("result", {}).values():
                address = value["address"]
                if isinstance(address, str):
                    mac_address = address
                    address_bytes = [int(x, 16) for x in address.split(":")]
                else:
                    address_bytes = address
                    mac_address = ":".join(f"{b:02X}" for b in address)

                result[
                    MacAddress(mac_address)
                ] = bluetooth_types.BluetoothPeerInfo(
                    id=f_bt.PeerId(value=int(value["id"])),
                    address=address_bytes,
                    connected=value["connected"],
                    bonded=value["bonded"],
                    name=value.get("name"),
                )
            return result
        except sl4f_errors.Sl4fError as e:
            raise bluetooth_errors.BluetoothError(
                "Failed to complete get_known_remote_devices SL4F call on "
                f"{self._name}."
            ) from e

    async def pair_device(
        self,
        identifier: f_bt.PeerId,
        connection_type: bluetooth_types.BluetoothConnectionType,
        timeout_sec: float | None = None,
    ) -> None:
        """Pair device to target remote device via Bluetooth.

        Args:
            identifier: the identifier of target remote device.
            connection_type: type of bluetooth connection
            timeout_sec: timeout duration in seconds

        Raises:
            BluetoothError: On failure.
        """
        try:
            self._sl4f.run(
                method=Sl4fMethods.PAIR_DEVICE,
                params={
                    "identifier": str(identifier.value),
                    "transport": connection_type.value,
                },
            )
        except sl4f_errors.Sl4fError as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete pair_device SL4F call on {self._name}."
            ) from e

    async def request_discovery(self, discovery: bool) -> None:
        """Requests Bluetooth Discovery on Bluetooth capable device.

        Args:
            discovery: True to start discovery, False to stop discovery.

        Raises:
            BluetoothError: On failure.
        """
        try:
            self._sl4f.run(
                method=Sl4fMethods.REQUEST_DISCOVERY,
                params={"discovery": discovery},
            )
        except sl4f_errors.Sl4fError as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete request_discovery SL4F call on {self._name}."
            ) from e

    async def set_discoverable(
        self,
        discoverable: bool,
    ) -> None:
        """Sets device to be discoverable by others.

        Args:
            discoverable: True to be discoverable by others, False to be not
                          discoverable by others.

        Raises:
            BluetoothError: On failure.
        """
        try:
            self._sl4f.run(
                method=Sl4fMethods.SET_DISCOVERABLE,
                params={"discoverable": discoverable},
            )
        except sl4f_errors.Sl4fError as e:
            raise bluetooth_errors.BluetoothError(
                f"Failed to complete set_discoverable SL4F call on {self._name}."
            ) from e

    async def run_pairing_delegate(
        self, timeout_sec: float | None = None
    ) -> None:
        """Function to run pairing delegate server calls.
        Args:
            timeout_sec: timeout duration in seconds

        Fuchsia Controller only implementation
        """
