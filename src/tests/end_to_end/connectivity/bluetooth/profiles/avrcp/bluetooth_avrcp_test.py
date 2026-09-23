#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Bluetooth AVRCP Test"""
import asyncio
import logging
from typing import List, Tuple

import fidl_fuchsia_bluetooth as f_bt
import fuchsia_base_test
from bluetooth_utils_lib import bluetooth_utils
from honeydew.affordances.connectivity.bluetooth.utils.types import (
    BluetoothAcceptPairing,
    BluetoothAvrcpCommand,
    BluetoothConnectionType,
)
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class MultipleFuchsiaDevicesNotFound(Exception):
    """When there are less than two Fuchsia devices available."""


class BluetoothAvrcpTest(fuchsia_base_test.FuchsiaBaseTest):
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
                "Two FuchsiaDevices are" "required to run BluetoothAvrcpTest"
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
        9. Initialize Avrcp service from Receiver
        10. Publish Media Mock Player from Initiator
        11. Send Pause command from Receiver to Initiator
        12. Verify that Pause command was received by Initiator
        13. Send Play command from Receiver to Initiator
        14. Verify that Play command was received by Initiator
        """

        _LOGGER.info(
            "Starting the Bluetooth AVRCP test iteration# %s", iteration
        )
        _LOGGER.info("Initializing Bluetooth and setting discoverability")
        await self.initiator.bluetooth_avrcp.request_discovery(True)
        await self.initiator.bluetooth_avrcp.set_discoverable(True)
        await self.receiver.bluetooth_avrcp.request_discovery(True)
        await self.receiver.bluetooth_avrcp.set_discoverable(True)
        # TODO(b/309011914): Remove sleep once polling for discoverability is added.
        await asyncio.sleep(3)

        receiver_address = (
            await self.receiver.bluetooth_avrcp.get_active_adapter_address()
        )
        _LOGGER.info("Receiver address: %s", receiver_address)
        await self.initiator.bluetooth_avrcp.accept_pairing(
            input_mode=BluetoothAcceptPairing.DEFAULT_INPUT_MODE,
            output_mode=BluetoothAcceptPairing.DEFAULT_OUTPUT_MODE,
        )
        await self.receiver.bluetooth_avrcp.accept_pairing(
            input_mode=BluetoothAcceptPairing.DEFAULT_INPUT_MODE,
            output_mode=BluetoothAcceptPairing.DEFAULT_OUTPUT_MODE,
        )
        _LOGGER.info(
            "Sleep for 5 seconds to wait for dut to listen for receiever"
        )
        await asyncio.sleep(5)

        known_devices = (
            await self.initiator.bluetooth_avrcp.get_known_remote_devices()
        )
        peer = known_devices[receiver_address]
        identifier = peer.id
        _LOGGER.info("Identifier: %s", identifier)
        _LOGGER.info("Attempting to initiate pairing")
        await self.initiator.bluetooth_avrcp.pair_device(
            identifier=identifier,
            connection_type=BluetoothConnectionType.CLASSIC,
        )
        await asyncio.sleep(5)

        await self.initiator.bluetooth_gap.connect_device(
            identifier=identifier,
            connection_type=BluetoothConnectionType.CLASSIC,
        )
        asserts.assert_true(
            await bluetooth_utils.verify_bt_pairing(
                identifier=identifier, device=self.initiator
            ),
            msg="Receiver was not paired.",
        )
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
            "Successfully ended the Bluetooth Sample test iteration# %s",
            iteration,
        )

        connected = await self.receiver.bluetooth_avrcp.get_connected_devices()
        _LOGGER.info("Initializing AVRCP service to ID: %s", connected[-1])
        await self.receiver.bluetooth_avrcp.init_avrcp(
            target_id=f_bt.PeerId(value=int(connected[-1]))
        )
        await asyncio.sleep(5)
        await self.initiator.bluetooth_avrcp.publish_mock_player()
        await asyncio.sleep(5)
        _LOGGER.info("Sending Pause command to AVRCP Source.")
        await self.receiver.bluetooth_avrcp.send_avrcp_command(
            command=BluetoothAvrcpCommand.PAUSE
        )
        await asyncio.sleep(5)
        _LOGGER.info("Checking if Pause command was sent to AVRCP Source.")
        received_requests = (
            await self.initiator.bluetooth_avrcp.list_received_requests()
        )
        asserts.assert_equal(
            received_requests[-1],
            "pause",
            msg="AVRCP Pause command not received",
        )
        _LOGGER.info("Sending Play command to AVRCP Source.")
        await self.receiver.bluetooth_avrcp.send_avrcp_command(
            command=BluetoothAvrcpCommand.PLAY
        )
        await asyncio.sleep(5)
        _LOGGER.info("Checking if Play command was sent to AVRCP Source.")
        received_requests = (
            await self.initiator.bluetooth_avrcp.list_received_requests()
        )
        asserts.assert_equal(
            received_requests[-1], "play", msg="AVRCP Play command not received"
        )
        _LOGGER.info(
            "AVRCP commands sent successfully. "
            "Successfully ended the Bluetooth AVRCP test iteration# %s",
            iteration,
        )

    async def teardown_test(self) -> None:
        #### Teardown
        _LOGGER.info("Removing all paired devices and " "turning off Bluetooth")
        await self.initiator.bluetooth_avrcp.stop_mock_player()
        await bluetooth_utils.forget_all_bt_devices(self.initiator)
        await bluetooth_utils.forget_all_bt_devices(self.receiver)
        await self.initiator.bluetooth_avrcp.set_discoverable(False)
        await self.receiver.bluetooth_avrcp.set_discoverable(False)
        return await super().teardown_test()

    def _name_func(self, iteration: int) -> str:
        """This function generates the names of each test case based on each
        argument set.

        The name function should have the same signature as the actual test
        logic function.

        Returns:
            Test case name
        """
        return f"test_bluetooth_avrcp_test_{iteration}"


if __name__ == "__main__":
    test_runner.main()
