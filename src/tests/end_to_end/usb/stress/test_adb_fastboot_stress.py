# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""ADB/Fastboot Transition Stress Test."""

import logging

import fuchsia_base_test
from mobly import signals, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class AdbFastbootStressTest(fuchsia_base_test.FuchsiaBaseTest):
    """Mobly test for loop/stress testing ADB reboot-bootloader and fastboot continue.

    Required Mobly Test Params:
        num_iterations (int): Number of times to execute the transition.
    """

    async def pre_run(self) -> None:
        test_arg_tuple_list: list[tuple[int]] = []
        num_iterations = int(self.user_params.get("num_iterations", 10))
        for iteration in range(1, num_iterations + 1):
            test_arg_tuple_list.append((iteration,))

        self.generate_tests(
            test_logic=self._test_logic,
            name_func=self._name_func,
            arg_sets=test_arg_tuple_list,
        )

    async def setup_class(self) -> None:
        await super().setup_class()
        self._adb_supported = await self.dut.adb.is_supported()
        if not self._adb_supported:
            _LOGGER.info("ADB is not supported at runtime on this device")
            return

        self._serial = self.dut.serial_number
        _LOGGER.info(f"Device serial number: {self._serial}")

    async def setup_test(self) -> None:
        await super().setup_test()
        if not self._adb_supported:
            raise signals.TestSkip("ADB is not supported in this build")

    async def _test_logic(self, iteration: int) -> None:
        _LOGGER.info(
            "Starting the ADB/Fastboot transition test iteration# %s", iteration
        )

        # Ensure device is online in Fuchsia
        await self.dut.wait_for_online()

        _LOGGER.info("Rebooting to bootloader via ADB...")
        self.dut.ffx.notify_intentional_disconnect()
        # adb reboot-bootloader usually returns immediately or quickly
        await self.dut.adb.run(["reboot-bootloader"])

        _LOGGER.info("Waiting for device to enter fastboot mode...")
        await self.dut.fastboot.wait_for_fastboot_mode()

        _LOGGER.info("Sending fastboot continue...")
        # fastboot continue might not return output if it succeeds and boots
        await self.dut.fastboot.run(["continue"])

        _LOGGER.info("Waiting for device to boot back to Fuchsia...")
        # We need to wait for Fuchsia mode (RCS) and online
        await self.dut.fastboot.wait_for_fuchsia_mode()
        await self.dut.wait_for_online()

        _LOGGER.info(
            "Successfully ended the ADB/Fastboot transition test iteration# %s",
            iteration,
        )

    def _name_func(self, iteration: int) -> str:
        return f"test_adb_fastboot_transition_{iteration}"


if __name__ == "__main__":
    test_runner.main()
