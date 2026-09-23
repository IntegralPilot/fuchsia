# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for Bluetooth affordance."""

import logging

import fidl_fuchsia_bluetooth as f_bt
import fuchsia_base_test
from honeydew.affordances.connectivity.bluetooth.utils import types as bt_types
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

BluetoothAcceptPairing = bt_types.BluetoothAcceptPairing
BluetoothConnectionType = bt_types.BluetoothConnectionType


class BluetoothGapAffordanceTests(fuchsia_base_test.FuchsiaBaseTest):
    """BluetoothGap affordance tests"""

    async def test_bluetooth_accept_pairing(self) -> None:
        """Test case for bluetooth.accept_pairing()"""

        input_mode = BluetoothAcceptPairing.DEFAULT_INPUT_MODE
        output_mode = BluetoothAcceptPairing.DEFAULT_OUTPUT_MODE

        await self.dut.bluetooth_gap.accept_pairing(input_mode, output_mode)

    async def test_bluetooth_connect_device(self) -> None:
        """Test case for bluetooth.connect_device()"""

        identifier = f_bt.PeerId(value=0)
        transport = BluetoothConnectionType.CLASSIC
        await self.dut.bluetooth_gap.connect_device(identifier, transport)

    async def test_bluetooth_forget_device(self) -> None:
        """Test case for bluetooth.forget_device()"""

        identifier = f_bt.PeerId(value=0)

        await self.dut.bluetooth_gap.forget_device(identifier)

    async def test_get_active_adapter_address(self) -> None:
        """Test case for bluetooth.get_active_adapter_address()"""
        await self.dut.bluetooth_gap.get_active_adapter_address()

    async def test_bluetooth_get_connected_devices(self) -> None:
        """Test case for bluetooth.get_connected_devices()"""

        res = await self.dut.bluetooth_gap.get_connected_devices()
        asserts.assert_equal(res, [])

    async def test_bluetooth_get_known_remote_devices(self) -> None:
        """Test case for bluetooth.get_known_remote_devices()"""

        await self.dut.bluetooth_gap.get_known_remote_devices()

    async def test_bluetooth_pair_device(self) -> None:
        """Test case for bluetooth.pair_device()"""

        identifier = f_bt.PeerId(value=0)
        transport = BluetoothConnectionType.CLASSIC

        await self.dut.bluetooth_gap.pair_device(identifier, transport)

    async def test_bluetooth_request_discovery(self) -> None:
        """Test case for bluetooth.request_discovery()"""

        await self.dut.bluetooth_gap.request_discovery(True)

    async def test_bluetooth_set_discoverable(self) -> None:
        """Test case for bluetooth_gap.set_discoverable()"""

        await self.dut.bluetooth_gap.set_discoverable(True)


if __name__ == "__main__":
    test_runner.main()
