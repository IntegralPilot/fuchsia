#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Bluetooth Smoke Test"""
import asyncio
import logging
from typing import List, Tuple

import fuchsia_base_test
from mobly import test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class MultipleFuchsiaDevicesNotFound(Exception):
    """When there are less than two Fuchsia devices available."""


class BluetoothSmokeTest(fuchsia_base_test.FuchsiaBaseTest):
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

    async def _test_logic(self, iteration: int) -> None:
        """Test Logic for Bluetooth Smoke Test
        1. Turn on BT discoverability on both devices
        2. Retrieve the receiver's BT address
        """
        _LOGGER.info("Initializing Bluetooth and setting discoverability")
        await self.dut.bluetooth_gap.request_discovery(True)
        await self.dut.bluetooth_gap.set_discoverable(True)
        # TODO(b/309011914): Remove sleep once polling for discoverability is added.
        await asyncio.sleep(3)

        bt_address = await self.dut.bluetooth_gap.get_active_adapter_address()
        _LOGGER.info("Receiver address: %s", bt_address)
        _LOGGER.info(
            "Completed Bluetooth state checks. "
            "Successfully ended the Bluetooth Smoke test."
        )

    async def teardown_test(self) -> None:
        """Teardown Test logic
        1. Turn off discoverability on device.
        2. Turn off discovery on device.
        """

        await self.dut.bluetooth_gap.set_discoverable(False)
        await self.dut.bluetooth_gap.request_discovery(False)
        return await super().teardown_test()

    def _name_func(self, iteration: int) -> str:
        """This function generates the names of each test case based on each
        argument set.

        The name function should have the same signature as the actual test
        logic function.

        Returns:
            Test case name
        """
        return f"test_bluetooth_smoke_test_{iteration}"


if __name__ == "__main__":
    test_runner.main()
