# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Driver suspend stress test runner."""

import datetime
import logging

import driver_stress_lib
import fuchsia_base_test
from honeydew import errors
from mobly import signals, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class DriverSuspendStressTest(fuchsia_base_test.FuchsiaBaseTest):
    """Driver power suspend/resume stress test runner.

    Attributes:
        dut: FuchsiaDevice object.

    Required Mobly Test Params:
        driver_url (str): Absolute component URL of the driver.
        iterations (int): Number of times suspend test need to be executed.
        devfs_paths (list[str]): Optional list of relative devfs paths to track.
    """

    async def pre_run(self) -> None:
        """Mobly method used to generate the test cases at run time."""
        iterations = int(self.user_params.get("iterations", 1))
        test_args: list[tuple[int]] = []

        for iteration in range(1, iterations + 1):
            test_args.append((iteration,))

        self.generate_tests(
            test_logic=self._test_logic,
            name_func=self._name_func,
            arg_sets=test_args,
        )

    async def setup_class(self) -> None:
        """setup_class is called once before running tests."""
        await super().setup_class()
        self.driver_url: str = self.user_params["driver_url"]
        self.devfs_paths: list[str] = self.user_params.get("devfs_paths", [])
        component = self.driver_url.split("#")[-1].split("/")[-1]
        self.moniker: str = component.replace(".cm", "")

    async def _test_logic(self, iteration: int) -> None:
        """Test case logic orchestrating system suspend loops."""
        start_time = datetime.datetime.now(datetime.timezone.utc).isoformat()
        _LOGGER.info("Starting driver suspend stress iteration# %s", iteration)

        for path in self.devfs_paths:
            await driver_stress_lib.assert_devfs_presence(
                dut=self.dut, path=path, expected=True
            )

        _LOGGER.info("Triggering system idle suspend for 5 seconds...")
        try:
            await self.dut.system_power_state_controller.idle_suspend_timer_based_resume(
                duration=5, verify_duration=True
            )
        except errors.NotSupportedError as err:
            _LOGGER.warning(
                "System power state controller affordance not supported on target!"
            )
            raise signals.TestSkip(
                "Target device/emulator does not support system power control affordance."
            ) from err

        await driver_stress_lib.verify_driver_loaded(
            dut=self.dut, driver_url=self.driver_url
        )

        for path in self.devfs_paths:
            await driver_stress_lib.assert_devfs_presence(
                dut=self.dut, path=path, expected=True
            )

        driver_stress_lib.audit_driver_crashes(
            dut=self.dut, moniker=self.moniker, start_time=start_time
        )

        _LOGGER.info(
            "Successfully finished driver suspend stress iteration# %s",
            iteration,
        )

    def _name_func(self, iteration: int) -> str:
        """Generates individual iteration test case names."""
        return f"test_suspend_iteration_{iteration}"


if __name__ == "__main__":
    test_runner.main()
