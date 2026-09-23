# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Sample test that demonstrates the usage of 2 Fuchsia devices in one test"""
import asyncio
import logging

import fuchsia_base_test
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class MultipleFuchsiaDevicesNotFound(Exception):
    """When there are less than two Fuchsia devices available."""


class MultiDeviceSampleTest(fuchsia_base_test.FuchsiaBaseTest):
    """Sample test that uses multiple Fuchsia devices"""

    async def pre_run(self) -> None:
        """Mobly method used to generate the test cases at run time."""
        test_arg_tuple_list: list[tuple[int]] = []

        for iteration in range(1, int(self.user_params["num_iterations"]) + 1):
            test_arg_tuple_list.append((iteration,))

        self.generate_tests(
            test_logic=self._test_logic,
            name_func=self._name_func,
            arg_sets=test_arg_tuple_list,
        )

    def _name_func(self, iteration: int) -> str:
        return f"test_bluetooth_sample_test_{iteration}"

    async def setup_class(self) -> None:
        """Initialize all DUTs."""
        await super().setup_class()
        if len(self.fuchsia_devices) < 2:
            raise MultipleFuchsiaDevicesNotFound(
                "Two FuchsiaDevices are" "required to run BluetoothSampleTest"
            )
        self.initiator = self.fuchsia_devices[0]
        self.receiver = self.fuchsia_devices[1]

    async def _test_logic(self, iteration: int) -> None:
        """Test Logic for Bluetooth Sample Test
        1. Turn on BT discoverability on both devices
        2. Retrieve the receiver's BT address
        3. Receive all broadcasting BT devices on initiator side.
        4. Check that the receiver is broadcasting to initiator.
        """

        _LOGGER.info(
            "Starting the Bluetooth Sample test iteration# %s", iteration
        )
        _LOGGER.info("Initializing Bluetooth and setting discoverability")
        await self._set_discoverability_on()

        address = await self.receiver.bluetooth_gap.get_active_adapter_address()

        _LOGGER.info(
            "Sleep for 5 seconds to wait for dut to listen for receiver"
        )
        await asyncio.sleep(5)
        known_devices = (
            await self.initiator.bluetooth_gap.get_known_remote_devices()
        )
        asserts.assert_true(
            address in known_devices,
            msg="Receiver was not discovered.",
        )

    async def teardown_test(self) -> None:
        """Teardown test will turn off discoverability for all the devices."""
        _LOGGER.info("Turning off discoverability on all devices")
        await self.initiator.bluetooth_gap.set_discoverable(False)
        await self.receiver.bluetooth_gap.set_discoverable(False)

        return await super().teardown_test()

    async def _set_discoverability_on(self) -> None:
        """Turns on discoverability for the devices."""
        await self.initiator.bluetooth_gap.request_discovery(True)
        await self.initiator.bluetooth_gap.set_discoverable(True)
        await self.receiver.bluetooth_gap.request_discovery(True)
        await self.receiver.bluetooth_gap.set_discoverable(True)


if __name__ == "__main__":
    test_runner.main()
