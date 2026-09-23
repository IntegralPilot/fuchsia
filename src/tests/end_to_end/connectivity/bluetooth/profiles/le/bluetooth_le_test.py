#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Sample test that demonstrates the usage of 2 Fuchsia devices in one test"""
import asyncio
import logging

import fuchsia_base_test
from honeydew.affordances.connectivity.bluetooth.utils.types import (
    BluetoothAcceptPairing,
    BluetoothLEAppearance,
)
from mobly import test_runner

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
        self.central = self.fuchsia_devices[0]
        self.peripheral = self.fuchsia_devices[1]

    async def _test_logic(self, iteration: int) -> None:
        """Minimal Viable Code for BLE testing
        1. Start Pairing Delegate Server
        2. Advertise from Peripheral end
        3. Call le_scan from Central
        4. Call connect from the central
        5. Call get_known_remote_devices from central to verify "connect: True"
        """
        await self.peripheral.bluetooth_le.accept_pairing(
            input_mode=BluetoothAcceptPairing.DEFAULT_INPUT_MODE,
            output_mode=BluetoothAcceptPairing.DEFAULT_OUTPUT_MODE,
        )
        await self._set_discoverability_on()
        _LOGGER.info(
            "Starting the Bluetooth Sample test iteration# %s", iteration
        )
        _LOGGER.info("Initializing Bluetooth and setting discoverability")
        _LOGGER.info(self.peripheral.device_name)
        await self.peripheral.bluetooth_le.advertise(
            appearance=BluetoothLEAppearance.GLUCOSE_MONITOR,
            name=self.peripheral.device_name,
        )
        known_devices = await self.central.bluetooth_le.scan()
        _LOGGER.info(known_devices)
        _LOGGER.info("Finished scanning, now going to connect.")
        for k in known_devices:
            assert k.id_ is not None
            await self.central.bluetooth_le.connect(k.id_)
            _LOGGER.info("Finished attempting to connect")
            await self.peripheral.bluetooth_le.wait_for_connection()
        await asyncio.sleep(5)
        print(await self.central.bluetooth_le.get_known_remote_devices())

    async def teardown_test(self) -> None:
        """Teardown test will turn off discoverability for all the devices."""
        _LOGGER.info("Forgetting all connected devices on Central/Peripheral.")
        await self.central.bluetooth_le.reset_state()
        await self.peripheral.bluetooth_le.reset_state()
        return await super().teardown_test()

    async def _set_discoverability_on(self) -> None:
        """Turns on discoverability for the devices."""
        _LOGGER.info("TRYING TO SET DISCOVERABLE")
        await self.central.bluetooth_le.request_discovery(True)
        await self.central.bluetooth_le.set_discoverable(True)
        await self.peripheral.bluetooth_le.set_discoverable(True)


if __name__ == "__main__":
    test_runner.main()
