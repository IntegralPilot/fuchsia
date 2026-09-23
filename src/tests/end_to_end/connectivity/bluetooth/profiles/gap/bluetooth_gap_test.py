#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Bluetooth Gap Test"""
import asyncio
import logging
from typing import List, Tuple

import fuchsia_base_test
from bluetooth_utils_lib import bluetooth_utils
from honeydew.affordances.connectivity.bluetooth.utils.types import (
    BluetoothAcceptPairing,
    BluetoothConnectionType,
)
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class MultipleFuchsiaDevicesNotFound(Exception):
    """When there are less than two Fuchsia devices available."""


class BluetoothGapTest(fuchsia_base_test.FuchsiaBaseTest):
    async def pre_run(self) -> None:
        """Mobly method used to generate the test cases at run time."""
        test_arg_tuple_list: List[Tuple[int]] = []

        for iteration in range(1, int(self.user_params["num_iterations"]) + 1):
            test_arg_tuple_list.append((iteration,))

        self.generate_tests(
            test_logic=self._test_logic,
            name_func=self._name_func,
            arg_sets=test_arg_tuple_list,
        )

    async def setup_class(self) -> None:
        """Initialize all DUT(s)"""
        await super().setup_class()
        if len(self.fuchsia_devices) < 2:
            raise MultipleFuchsiaDevicesNotFound(
                "Two FuchsiaDevices are" "required to run BluetoothGapTest"
            )
        self.initiator = self.fuchsia_devices[0]
        self.receiver = self.fuchsia_devices[1]

    async def _test_logic(self, iteration: int) -> None:
        """Test Logic for Bluetooth Sample Test
        1. Turn on BT discoverability on both devices
        2. Retrieve the receiver's BT address
        3. Enable Pairing mode for both Initiator and Receiver
        3. Receive all advertising BT devices on initiator side.
        4. Check that the receiver is advertising to initiator.
        5. Initiate pairing from initiator to receiver.
        6. Verify that pairing was successful.
        7. Initiate connection from initiator to receiver.
        8. Verify that connection was successful.
        """

        _LOGGER.info("Starting the Bluetooth Gap test iteration# %s", iteration)
        _LOGGER.info("Initializing Bluetooth and setting discoverability")
        await self._set_discoverability_on()
        # TODO(b/309011914): Remove sleep once polling for discoverability is added.
        await asyncio.sleep(3)

        receiver_address = (
            await self.receiver.bluetooth_gap.get_active_adapter_address()
        )
        _LOGGER.info("Receiver address: %s", receiver_address)
        await self.initiator.bluetooth_gap.accept_pairing(
            input_mode=BluetoothAcceptPairing.DEFAULT_INPUT_MODE,
            output_mode=BluetoothAcceptPairing.DEFAULT_OUTPUT_MODE,
        )
        await self.receiver.bluetooth_gap.accept_pairing(
            input_mode=BluetoothAcceptPairing.DEFAULT_INPUT_MODE,
            output_mode=BluetoothAcceptPairing.DEFAULT_OUTPUT_MODE,
        )
        _LOGGER.info(
            "Sleep for 5 seconds to wait for dut to listen for receiever"
        )
        await asyncio.sleep(5)

        known_devices = (
            await self.initiator.bluetooth_gap.get_known_remote_devices()
        )
        _LOGGER.info(known_devices)
        peer = known_devices[receiver_address]
        identifier = peer.id
        _LOGGER.info("Identifier: %s", identifier)
        _LOGGER.info("Attempting to initiate pairing")
        await self.initiator.bluetooth_gap.pair_device(
            identifier=identifier,
            connection_type=BluetoothConnectionType.CLASSIC,
        )
        await asyncio.sleep(5)
        await self.initiator.bluetooth_gap.connect_device(
            identifier=identifier,
            connection_type=BluetoothConnectionType.CLASSIC,
        )
        await self.receiver.bluetooth_gap.run_pairing_delegate(None)
        await asyncio.sleep(5)

        _LOGGER.info("Attempting to start connection")
        await self.initiator.bluetooth_gap.connect_device(
            identifier=identifier,
            connection_type=BluetoothConnectionType.CLASSIC,
        )
        asserts.assert_true(
            await bluetooth_utils.verify_bt_connection(
                identifier=identifier, device=self.initiator
            ),
            msg="Receiver was not connected.",
        )
        _LOGGER.info(
            "Pairing and Connection complete. "
            "Successfully ended the Bluetooth GAP test iteration# %s",
            iteration,
        )

    async def teardown_class(self) -> None:
        """Teardown Test logic
        1. Forget all paired devices from initiator.
        2. Forget all paired devices from receiver.
        3. Turn off discoverability on initiator.
        4. Turn off discoverability on receiver.
        """

        _LOGGER.info("Removing all paired devices and " "turning off Bluetooth")
        # TODO: b/372749232: Debug bluetooth.sys.access FIDL
        # bluetooth_utils.forget_all_bt_devices(self.initiator)
        await self.initiator.bluetooth_gap.set_discoverable(False)
        await self.receiver.bluetooth_gap.set_discoverable(False)
        return await super().teardown_class()

    def _name_func(self, iteration: int) -> str:
        """This function generates the names of each test case based on each
        argument set.

        The name function should have the same signature as the actual test
        logic function.

        Returns:
            Test case name
        """
        return f"test_bluetooth_gap_test_{iteration}"

    async def _set_discoverability_on(self) -> None:
        """Turns on discoverability for the devices."""
        await self.initiator.bluetooth_gap.request_discovery(True)
        await self.initiator.bluetooth_gap.set_discoverable(True)
        await self.receiver.bluetooth_gap.request_discovery(True)
        await self.receiver.bluetooth_gap.set_discoverable(True)


if __name__ == "__main__":
    test_runner.main()
